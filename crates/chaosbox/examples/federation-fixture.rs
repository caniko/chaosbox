//! Deterministic, synthetic admission fixtures for disposable federation tests.
//! No inference service or owner-local data is read by this helper.
use std::{fs, path::Path};

use chaosbox::intelligence::Bundle;
#[path = "../tests/support/federation.rs"]
mod fixture;

fn write(path: &Path, bundle: &Bundle) {
    bundle.validate().unwrap();
    fs::write(path, serde_json::to_vec(bundle).unwrap()).unwrap();
}

fn main() {
    let directory = std::env::args()
        .nth(1)
        .expect("fixture output directory required");
    let root = Path::new(&directory);
    fs::create_dir_all(root).unwrap();
    for (owner, repo, statement) in [
        (
            "can",
            "can-local",
            "We must preserve bounded context citations.",
        ),
        (
            "dejana",
            "dejana-local",
            "We must keep context citations independently attributed.",
        ),
    ] {
        let old = fixture::bundle(
            owner,
            &[
                ("shared", statement, repo),
                (
                    "private",
                    "We must keep PRIVATE-OTHER-PROJECT context secret.",
                    "other-local",
                ),
            ],
        );
        write(&root.join(format!("{owner}.json")), &old);
        let history = root.join(format!("{owner}-history"));
        fs::create_dir_all(&history).unwrap();
        write(
            &history.join(format!("{}.json", old.digest().unwrap())),
            &old,
        );
        let next = fixture::bundle(
            owner,
            &[
                ("shared", statement, repo),
                (
                    "private",
                    "We must keep PRIVATE-OTHER-PROJECT context secret.",
                    "other-local",
                ),
                ("next", "We must retain new context independently.", repo),
            ],
        );
        write(&root.join(format!("{owner}-next.json")), &next);
        let mut withheld = next;
        fixture::withhold(&mut withheld, "shared", statement, repo);
        write(&root.join(format!("{owner}-withheld.json")), &withheld);
    }
}
