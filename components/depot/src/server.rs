// Copyright:: Copyright (c) 2015-2016 The Habitat Maintainers
//
// The terms of the Evaluation Agreement (Habitat) between Chef Software Inc.
// and the party accessing this file ("Licensee") apply to Licensee's use of
// the Software until such time that the Software is made available under an
// open source license such as the Apache 2.0 License.

use std::fs::{self, File};
use std::io::{Read, Write, BufWriter};
use std::path::PathBuf;

use dbcache;
use depot_core::data_object::{self, DataObject};
use iron::prelude::*;
use iron::{status, headers, AfterMiddleware};
use iron::request::Body;
use mount::Mount;
use router::{Params, Router};
use rustc_serialize::json;
use urlencoded::UrlEncodedQuery;


use super::Depot;
use config::Config;
use error::{Error, Result};
use hcore::package::{self, PackageArchive};

fn write_file(filename: &PathBuf, body: &mut Body) -> Result<bool> {
    let path = filename.parent().unwrap();
    try!(fs::create_dir_all(path));
    let tempfile = format!("{}.tmp", filename.to_string_lossy());
    let f = try!(File::create(&tempfile));
    let mut writer = BufWriter::new(&f);
    let mut written: i64 = 0;
    let mut buf = [0u8; 100000]; // Our byte buffer
    loop {
        let len = try!(body.read(&mut buf)); // Raise IO errors
        match len {
            0 => {
                // 0 == EOF, so stop writing and finish progress
                break;
            }
            _ => {
                // Write the buffer to the BufWriter on the Heap
                let bytes_written = try!(writer.write(&buf[0..len]));
                if bytes_written == 0 {
                    return Err(Error::WriteSyncFailed);
                }
                written = written + (bytes_written as i64);
            }
        };
    }
    info!("File added to Depot at {}", filename.to_string_lossy());
    try!(fs::rename(&tempfile, &filename));
    Ok(true)
}

fn upload_origin_key(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    debug!("Upload Origin Key {:?}", req);
    let params = req.extensions.get::<Router>().unwrap();

    let origin = match params.find("origin") {
        Some(origin) => origin,
        None => return Ok(Response::with(status::BadRequest)),
    };

    let revision = match params.find("revision") {
        Some(revision) => revision,
        None => return Ok(Response::with(status::BadRequest)),
    };

    let origin_keyfile = depot.key_path(&origin, &revision);
    debug!("Writing key file {}", origin_keyfile.to_string_lossy());
    if origin_keyfile.is_file() {
        return Ok(Response::with(status::Conflict));
    }

    depot.datastore.origin_keys.write(&origin, &revision).unwrap();

    try!(write_file(&origin_keyfile, &mut req.body));

    let mut response = Response::with((status::Created,
                                       format!("/origins/{}/keys/{}", &origin, &revision)));

    let mut base_url = req.url.clone();
    base_url.path = vec![String::from("key"), format!("{}-{}", &origin, &revision)];
    response.headers.set(headers::Location(format!("{}", base_url)));
    Ok(response)
}

fn upload_origin_secret_key(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    debug!("Upload Origin Secret Key {:?}", req);
    let params = req.extensions.get::<Router>().unwrap();

    let origin = match params.find("origin") {
        Some(origin) => origin,
        None => return Ok(Response::with(status::BadRequest)),
    };

    let revision = match params.find("revision") {
        Some(revision) => revision,
        None => return Ok(Response::with(status::BadRequest)),
    };
    debug!("Origin = {}, revision = {}", &origin, &revision);

    if !try!(depot.datastore.origin_keys.exists(&origin, &revision)) {
        debug!("Public key doesn't exist for this origin and revision");
        return Ok(Response::with(status::NotFound));
    }

    let mut content = String::new();
    // TODO: unwrap/error stuff
    req.body.read_to_string(&mut content).unwrap();
    // we don't actually need a revision here, but if anything changes 
    // regarding the storage of the key, it's nice to have it plumbed through already
    depot.datastore.origin_secret_keys.write(&origin, &revision, &content).unwrap();

    // TODO: this response doesn't/won't have a Location
    let mut response = Response::with((status::Created,
                                       format!("/origins/{}/keys/{}", &origin, &revision)));
    Ok(response)
}

fn upload_package(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    debug!("Upload {:?}", req);
    let checksum_from_param = match extract_query_value("checksum", req) {
        Some(checksum) => checksum,
        None => return Ok(Response::with(status::BadRequest)),
    };
    let params = req.extensions.get::<Router>().unwrap();
    let ident: package::PackageIdent = extract_ident(params);

    if !ident.fully_qualified() {
        return Ok(Response::with(status::BadRequest));
    }

    match depot.datastore.packages.get(&ident) {
        Ok(_) |
        Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
            if let Some(_) = depot.archive(&ident) {
                return Ok(Response::with((status::Conflict)));
            }
        }
        Err(e) => {
            error!("upload_package:1, err={:?}", e);
            return Ok(Response::with(status::InternalServerError));
        }
    }

    let filename = depot.archive_path(&ident);
    try!(write_file(&filename, &mut req.body));
    let mut archive = PackageArchive::new(filename);
    debug!("Package Archive: {:#?}", archive);
    let checksum_from_artifact = match archive.checksum() {
        Ok(cksum) => cksum,
        Err(e) => {
            info!("Could not compute a checksum for {:#?}: {:#?}", archive, e);
            return Ok(Response::with(status::UnprocessableEntity));
        }
    };
    if checksum_from_param != checksum_from_artifact {
        info!("Checksums did not match: from_param={:?}, from_artifact={:?}",
              checksum_from_param,
              checksum_from_artifact);
        return Ok(Response::with(status::UnprocessableEntity));
    }
    let object = match data_object::Package::from_archive(&mut archive) {
        Ok(object) => object,
        Err(e) => {
            info!("Error building package from archive: {:#?}", e);
            return Ok(Response::with(status::UnprocessableEntity));
        }
    };
    if ident.satisfies(&object.ident) {
        depot.datastore.packages.write(&object).unwrap();
        let mut response = Response::with((status::Created,
                                           format!("/pkgs/{}/download", object.ident)));
        let mut base_url = req.url.clone();
        base_url.path = vec![String::from("pkgs"),
                             object.ident.to_string(),
                             String::from("download")];
        response.headers.set(headers::Location(format!("{}", base_url)));
        Ok(response)
    } else {
        info!("Ident mismatch, expected={:?}, got={:?}",
              ident,
              &object.ident);
        Ok(Response::with(status::UnprocessableEntity))
    }
}

fn download_origin_key(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    debug!("Download origin key {:?}", req);
    let params = req.extensions.get::<Router>().unwrap();

    let origin = match params.find("origin") {
        Some(origin) => origin,
        None => return Ok(Response::with(status::BadRequest)),
    };

    let revision = match params.find("revision") {
        Some(revision) => revision,
        None => return Ok(Response::with(status::BadRequest)),
    };
    debug!("Trying to retreive origin key {}-{}", &origin, &revision);
    let origin_keyfile = depot.key_path(&origin, &revision);
    debug!("Looking for {}", &origin_keyfile.to_string_lossy());
    match origin_keyfile.metadata() {
        Ok(md) => {
            if !md.is_file() {
                return Ok(Response::with(status::NotFound));
            };
        }
        Err(e) => {
            println!("Can't read key file {}: {}",
                     &origin_keyfile.to_string_lossy(),
                     e);
            return Ok(Response::with(status::NotFound));
        }
    };

    let xfilename = origin_keyfile.file_name().unwrap().to_string_lossy().into_owned();
    let mut response = Response::with((status::Ok, origin_keyfile));
    // use set_raw because we're having problems with Iron's Hyper 0.8.x
    // and the newer Hyper 0.9.4. TODO: change back to set() once
    // Iron updates to Hyper 0.9.x.
    response.headers.set_raw("X-Filename", vec![xfilename.clone().into_bytes()]);
    response.headers.set_raw("content-disposition",
                             vec![format!("attachment; filename=\"{}\"", xfilename.clone())
                                      .into_bytes()]);
    Ok(response)
}


fn download_latest_origin_key(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    debug!("Download latest origin key {:?}", req);
    let params = req.extensions.get::<Router>().unwrap();

    let origin = match params.find("origin") {
        Some(origin) => origin,
        None => return Ok(Response::with(status::BadRequest)),
    };
    debug!("Trying to retreive latest origin key for {}", &origin);
    let latest_rev = depot.datastore.origin_keys.latest(&origin).unwrap();
    let origin_keyfile = depot.key_path(&origin, &latest_rev);
    debug!("Looking for {}", &origin_keyfile.to_string_lossy());
    match origin_keyfile.metadata() {
        Ok(md) => {
            if !md.is_file() {
                return Ok(Response::with(status::NotFound));
            };
        }
        Err(e) => {
            println!("Can't read key file {}: {}",
                     &origin_keyfile.to_string_lossy(),
                     e);
            return Ok(Response::with(status::NotFound));
        }
    };

    let xfilename = origin_keyfile.file_name().unwrap().to_string_lossy().into_owned();
    let mut response = Response::with((status::Ok, origin_keyfile));
    // use set_raw because we're having problems with Iron's Hyper 0.8.x
    // and the newer Hyper 0.9.4. TODO: change back to set() once
    // Iron updates to Hyper 0.9.x.
    response.headers.set_raw("X-Filename", vec![xfilename.clone().into_bytes()]);
    response.headers.set_raw("content-disposition",
                             vec![format!("attachment; filename=\"{}\"", xfilename.clone())
                                      .into_bytes()]);
    Ok(response)
}

fn download_package(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    debug!("Download {:?}", req);
    let params = req.extensions.get::<Router>().unwrap();
    let ident: data_object::PackageIdent = extract_data_ident(params);

    match depot.datastore.packages.get(&ident) {
        Ok(ident) => {
            if let Some(archive) = depot.archive(&ident) {
                match fs::metadata(&archive.path) {
                    Ok(_) => {
                        let mut response = Response::with((status::Ok, archive.path.clone()));
                        // use set_raw because we're having problems with Iron's Hyper 0.8.x
                        // and the newer Hyper 0.9.4. TODO: change back to set() once
                        // Iron updates to Hyper 0.9.x.

                        response.headers.set_raw("X-Filename",
                                                 vec![archive.file_name().clone().into_bytes()]);
                        response.headers.set_raw("content-disposition",
                                                 vec![format!("attachment; filename=\"{}\"",
                                                              archive.file_name().clone())
                                                          .into_bytes()]);
                        Ok(response)
                    }
                    Err(_) => Ok(Response::with(status::NotFound)),
                }
            } else {
                // This should never happen. Writing the package to disk and recording it's existence
                // in the metadata is a transactional operation and one cannot exist without the other.
                panic!("Inconsistent package metadata! Exit and run `hab-depot repair` to fix data integrity.");
            }
        }
        Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
            Ok(Response::with((status::NotFound)))
        }
        Err(e) => {
            error!("download_package:1, err={:?}", e);
            Ok(Response::with(status::InternalServerError))
        }
    }
}

fn list_origin_keys(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    let params = req.extensions.get::<Router>().unwrap();
    let origin = match params.find("origin") {
        Some(origin) => origin,
        None => return Ok(Response::with(status::BadRequest)),
    };

    match depot.datastore.origin_keys.all(origin) {
        Ok(revisions) => {
            let body = json::encode(&revisions).unwrap();
            Ok(Response::with((status::Ok, body)))
        }
        Err(e) => {
            error!("list_origin_keys:1, err={:?}", e);
            Ok(Response::with(status::InternalServerError))
        }
    }

}

fn list_packages(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    let params = req.extensions.get::<Router>().unwrap();
    let ident: String = if params.find("pkg").is_none() {
        match params.find("origin") {
            Some(origin) => origin.to_string(),
            None => return Ok(Response::with(status::BadRequest)),
        }
    } else {
        extract_data_ident(params).ident().to_owned()
    };

    if let Some(view) = params.find("view") {
        match depot.datastore.views.view_pkg_idx.all(view, &ident) {
            Ok(packages) => {
                let body = json::encode(&packages).unwrap();
                Ok(Response::with((status::Ok, body)))
            }
            Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
                Ok(Response::with((status::NotFound)))
            }
            Err(e) => {
                error!("list_packages:1, err={:?}", e);
                Ok(Response::with(status::InternalServerError))
            }
        }
    } else {
        match depot.datastore.packages.index.all(&ident) {
            Ok(packages) => {
                let body = json::encode(&packages).unwrap();
                Ok(Response::with((status::Ok, body)))
            }
            Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
                Ok(Response::with((status::NotFound)))
            }
            Err(e) => {
                error!("list_packages:2, err={:?}", e);
                Ok(Response::with(status::InternalServerError))
            }
        }
    }
}

fn list_views(depot: &Depot, _req: &mut Request) -> IronResult<Response> {
    let views = try!(depot.datastore.views.all());
    let body = json::encode(&views).unwrap();
    Ok(Response::with((status::Ok, body)))
}

fn show_package(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    let params = req.extensions.get::<Router>().unwrap();
    let mut ident: data_object::PackageIdent = extract_data_ident(params);

    if let Some(view) = params.find("view") {
        if !ident.fully_qualified() {
            match depot.datastore.views.view_pkg_idx.latest(view, &ident.to_string()) {
                Ok(ident) => {
                    match depot.datastore.packages.get(&ident) {
                        Ok(pkg) => render_package(&pkg),
                        Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
                            Ok(Response::with(status::NotFound))
                        }
                        Err(e) => {
                            error!("show_package:1, err={:?}", e);
                            Ok(Response::with(status::InternalServerError))
                        }
                    }
                }
                Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
                    Ok(Response::with(status::NotFound))
                }
                Err(e) => {
                    error!("show_package:2, err={:?}", e);
                    Ok(Response::with(status::InternalServerError))
                }
            }
        } else {
            match depot.datastore.views.view_pkg_idx.is_member(view, &ident) {
                Ok(true) => {
                    match depot.datastore.packages.get(&ident) {
                        Ok(pkg) => render_package(&pkg),
                        Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
                            Ok(Response::with(status::NotFound))
                        }
                        Err(e) => {
                            error!("show_package:3, err={:?}", e);
                            Ok(Response::with(status::InternalServerError))
                        }
                    }
                }
                Ok(false) => Ok(Response::with(status::NotFound)),
                Err(e) => {
                    error!("show_package:4, err={:?}", e);
                    Ok(Response::with(status::InternalServerError))
                }
            }
        }
    } else {
        if !ident.fully_qualified() {
            match depot.datastore.packages.index.latest(&ident) {
                Ok(id) => ident = id.into(),
                Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
                    return Ok(Response::with(status::NotFound));
                }
                Err(e) => {
                    error!("show_package:5, err={:?}", e);
                    return Ok(Response::with(status::InternalServerError));
                }
            }
        }

        match depot.datastore.packages.get(&ident) {
            Ok(pkg) => render_package(&pkg),
            Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
                Ok(Response::with(status::NotFound))
            }
            Err(e) => {
                error!("show_package:6, err={:?}", e);
                Ok(Response::with(status::InternalServerError))
            }
        }
    }
}


fn render_package(pkg: &data_object::Package) -> IronResult<Response> {
    let body = json::encode(pkg).unwrap();
    let mut response = Response::with((status::Ok, body));
    // use set_raw because we're having problems with Iron's Hyper 0.8.x
    // and the newer Hyper 0.9.4. TODO: change back to set() once
    // Iron updates to Hyper 0.9.x.
    response.headers.set_raw("ETag", vec![pkg.checksum.clone().into_bytes()]);
    Ok(response)
}

fn promote_package(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    let params = req.extensions.get::<Router>().unwrap();
    let view = params.find("view").unwrap();

    match depot.datastore.views.is_member(view) {
        Ok(true) => {
            let ident: package::PackageIdent = extract_ident(params);
            match depot.datastore.packages.get(&ident) {
                Ok(package) => {
                    depot.datastore.views.associate(view, &package).unwrap();
                    Ok(Response::with(status::Ok))
                }
                Err(Error::DataStore(dbcache::Error::EntityNotFound)) => {
                    Ok(Response::with(status::NotFound))
                }
                Err(e) => {
                    error!("promote:2, err={:?}", e);
                    return Ok(Response::with(status::InternalServerError));
                }
            }
        }
        Ok(false) => Ok(Response::with(status::NotFound)),
        Err(e) => {
            error!("promote:1, err={:?}", e);
            return Ok(Response::with(status::InternalServerError));
        }
    }
}

fn extract_ident(params: &Params) -> package::PackageIdent {
    package::PackageIdent::new(params.find("origin").unwrap(),
                               params.find("pkg").unwrap(),
                               params.find("version"),
                               params.find("release"))
}

fn extract_data_ident(params: &Params) -> data_object::PackageIdent {
    let ident: package::PackageIdent = extract_ident(params);
    data_object::PackageIdent::new(ident)
}

fn extract_query_value(key: &str, req: &mut Request) -> Option<String> {
    match req.get_ref::<UrlEncodedQuery>() {
        Ok(map) => {
            for (k, v) in map.iter() {
                if key == *k {
                    if v.len() < 1 {
                        return None;
                    }
                    return Some(v[0].clone());
                }
            }
            None
        }
        Err(_) => None,
    }
}


fn create_origin(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    println!("Create origin {:?}", req);
    let params = req.extensions.get::<Router>().unwrap();

    let origin = match params.find("origin") {
        Some(origin) => origin,
        None => return Ok(Response::with(status::BadRequest)),
    };

    /*
    let owner = match params.find("user") {
        Some(owner) => owner,
        None => return Ok(Response::with(status::BadRequest)),
    };
    */
    let owner = "dparfitt";
    println!("Origin = {}, owner = {}", &origin, &owner);
    // TODO: hardcoded owner
    try!(depot.datastore.origins.create(&origin, &owner));
    let mut response = Response::with((status::Created,
                                       format!("/origins/{}/users/{}", &origin, &owner)));
    Ok(response)
}

//fn create_user(depot: &Depot, req: &mut Request) -> IronResult<Response> {
//}

fn delete_origin(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    println!("Delete origin {:?}", req);
    let params = req.extensions.get::<Router>().unwrap();
    let origin = match params.find("origin") {
        Some(origin) => origin,
        None => return Ok(Response::with(status::BadRequest)),
    };

    let mut response = Response::with((status::Ok));
    // TODO: who can delete?
    try!(depot.datastore.origins.delete(&origin));
    Ok(response)
}

fn add_user_to_origin(depot: &Depot, req: &mut Request) -> IronResult<Response> {
    println!("Add user to origin {:?}", req);
    let params = req.extensions.get::<Router>().unwrap();

    let origin = match params.find("origin") {
        Some(origin) => origin,
        None => return Ok(Response::with(status::BadRequest)),
    };

    let user = match params.find("user") {
        Some(user) => user,
        None => return Ok(Response::with(status::BadRequest)),
    };

    try!(depot.datastore.origins.add_member(&origin, &user));

    let mut response = Response::with((status::Ok));
    Ok(response)
}

fn remove_user_from_origin(depot: &Depot, req: &mut Request) -> IronResult<Response> {
     println!("Remove user from origin {:?}", req);
    let params = req.extensions.get::<Router>().unwrap();

    let origin = match params.find("origin") {
        Some(origin) => origin,
        None => return Ok(Response::with(status::BadRequest)),
    };

    let user = match params.find("user") {
        Some(user) => user,
        None => return Ok(Response::with(status::BadRequest)),
    };

    try!(depot.datastore.origins.delete_member(&origin, &user));

    let mut response = Response::with((status::Ok));
    Ok(response)
}


struct Cors;

impl AfterMiddleware for Cors {
    fn after(&self, _req: &mut Request, mut res: Response) -> IronResult<Response> {
        res.headers.set(headers::AccessControlAllowOrigin::Any);
        Ok(res)
    }
}

pub fn router(config: Config) -> Result<Chain> {
    let depot = try!(Depot::new(config));
    let depot1 = depot.clone();
    let depot2 = depot.clone();
    let depot3 = depot.clone();
    let depot4 = depot.clone();
    let depot5 = depot.clone();
    let depot6 = depot.clone();
    let depot7 = depot.clone();
    let depot8 = depot.clone();
    let depot9 = depot.clone();
    let depot10 = depot.clone();
    let depot11 = depot.clone();
    let depot12 = depot.clone();
    let depot13 = depot.clone();
    let depot14 = depot.clone();
    let depot15 = depot.clone();
    let depot16 = depot.clone();
    let depot17 = depot.clone();
    let depot18 = depot.clone();
    let depot19 = depot.clone();
    let depot20 = depot.clone();
    let depot21 = depot.clone();
    let depot22 = depot.clone();
    let depot23 = depot.clone();
    let depot24 = depot.clone();
    let depot25 = depot.clone();

    let router = router!(
        get "/views" => move |r: &mut Request| list_views(&depot1, r),
        get "/views/:view/pkgs/:origin" => move |r: &mut Request| list_packages(&depot2, r),
        get "/views/:view/pkgs/:origin/:pkg" => move |r: &mut Request| list_packages(&depot3, r),
        get "/views/:view/pkgs/:origin/:pkg/latest" => move |r: &mut Request| show_package(&depot4, r),
        get "/views/:view/pkgs/:origin/:pkg/:version" => move |r: &mut Request| list_packages(&depot5, r),
        get "/views/:view/pkgs/:origin/:pkg/:version/latest" => move |r: &mut Request| show_package(&depot6, r),
        get "/views/:view/pkgs/:origin/:pkg/:version/:release" => move |r: &mut Request| show_package(&depot7, r),

        post "/views/:view/pkgs/:origin/:pkg/:version/:release/promote" => move |r: &mut Request| promote_package(&depot8, r),

        get "/pkgs/:origin" => move |r: &mut Request| list_packages(&depot9, r),
        get "/pkgs/:origin/:pkg" => move |r: &mut Request| list_packages(&depot10, r),
        get "/pkgs/:origin/:pkg/latest" => move |r: &mut Request| show_package(&depot11, r),
        get "/pkgs/:origin/:pkg/:version" => move |r: &mut Request| list_packages(&depot12, r),
        get "/pkgs/:origin/:pkg/:version/latest" => move |r: &mut Request| show_package(&depot13, r),
        get "/pkgs/:origin/:pkg/:version/:release" => move |r: &mut Request| show_package(&depot14, r),

        get "/pkgs/:origin/:pkg/:version/:release/download" => move |r: &mut Request| download_package(&depot15, r),
        post "/pkgs/:origin/:pkg/:version/:release" => move |r: &mut Request| upload_package(&depot16, r),


        get "/origins/:origin/keys" => move |r: &mut Request| list_origin_keys(&depot17, r),
        get "/origins/:origin/keys/latest" => move |r: &mut Request| download_latest_origin_key(&depot19, r),
        get "/origins/:origin/keys/:revision" => move |r: &mut Request| download_origin_key(&depot18, r),

        post "/origins/:origin/keys/:revision" => move |r: &mut Request| upload_origin_key(&depot20, r),
        post "/origins/:origin/secret_keys/:revision" => move |r: &mut Request| upload_origin_secret_key(&depot21, r),

        // initial origin creation
        post   "/origins/:origin/users" => move |r: &mut Request| create_origin(&depot22, r),
        delete "/origins/:origin" => move |r: &mut Request| delete_origin(&depot23, r),
        // add user to origin
        put "/origins/:origin/users/:user" => move |r: &mut Request| add_user_to_origin(&depot24, r),
        // remove a user from an origin
        delete "/origins/:origin/users/:user" => move |r: &mut Request| remove_user_from_origin(&depot25, r)
        );
    let mut chain = Chain::new(router);
    chain.link_after(Cors);
    Ok(chain)
}

pub fn run(config: Config) -> Result<()> {
    let listen_addr = config.listen_addr.clone();
    let v1 = try!(router(config));
    let mut mount = Mount::new();
    mount.mount("/v1", v1);
    Iron::new(mount).http(listen_addr).unwrap();
    Ok(())
}

impl From<Error> for IronError {
    fn from(err: Error) -> IronError {
        IronError {
            error: Box::new(err),
            response: Response::with((status::InternalServerError, "Internal Habitat error")),
        }
    }
}
