// Copyright (c) 2016 Chef Software Inc. and/or applicable contributors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::env;
use std::path::PathBuf;

pub fn exe_path() -> PathBuf {
    env::current_exe().unwrap()
}

pub fn root() -> PathBuf {
    exe_path().parent().unwrap().parent().unwrap().parent().unwrap().join("tests")
}

pub fn fixtures() -> PathBuf {
    root().join("fixtures")
}

pub fn key_cache() -> PathBuf {
    // same as the fixtures dir, for now
    root().join("fixtures")
}


pub fn fixture(name: &str) -> PathBuf {
    fixtures().join(name)
}

pub fn fixture_as_string(name: &str) -> String {
    let fixture_string = fixtures().join(name).to_string_lossy().into_owned();
    fixture_string
}

pub fn plan_build() -> String {
    root()
        .parent()
        .unwrap()
        .join("components/plan-build/bin/hab-plan-build.sh")
        .to_string_lossy()
        .into_owned()
}

pub fn sup() -> String {
    root().parent().unwrap().join("target/debug/hab-sup").to_string_lossy().into_owned()
}
