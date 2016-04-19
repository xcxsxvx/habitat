// Copyright:: Copyright (c) 2015-2016 The Habitat Maintainers
//
// The terms of the Evaluation Agreement (Habitat) between Chef Software Inc.
// and the party accessing this file ("Licensee") apply to Licensee's use of
// the Software until such time that the Software is made available under an
// open source license such as the Apache 2.0 License.

import "whatwg-fetch";
import config from "./config";

const urlPrefix = config["builder_url"] || "";

export function createOrigin(name) {
    return new Promise((resolve, reject) => {
        // FIXME: Remove this when we have a real api
        resolve({ name });
        fetch(`${urlPrefix}/origins`, {
            body: JSON.stringify({ name }),
            method: "POST"
        }).then(response => {
            resolve(response.json());
        }).catch(error => reject(error));
    });
}

export function isOriginAvailable(name) {
    return new Promise((resolve, reject) => {
        fetch(`${urlPrefix}/origins/${name}`).then(response => {
            // FIXME: FAKE!
            if (name === "smith") { resolve(true); }

            // Getting a 200 means it exists and is already taken.
            if (response.status === 200) {
                reject(false);
            // Getting a 404 means it does not exist and is available.
            } else if (response.status === 404) {
                resolve(true);
            }
        }).catch(error => {
            // FIXME: FAKE!
            if (name === "smith") { resolve(true); }

            // This happens when there is a network error. We'll say that it is
            // not available.
            reject(false);
        });
    });
}