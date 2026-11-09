//! The committed JSON Schema and TypeScript files must match the Rust types.
//!
//! When a test fails, review the change, then run
//! `DYNO_UPDATE_GENERATED=1 INSTA_UPDATE=always cargo test --workspace` to rewrite the committed files.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use dyno_schema::generate::{self, UPDATE_GENERATED_VARIABLE};

/// The npm package `@dyno-labs/schema`: the JSON Schema in `json/` and the TypeScript types in
/// `ts/`, next to its `package.json`.
fn package_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/schema")
}

fn updating_generated_files() -> bool {
    std::env::var(UPDATE_GENERATED_VARIABLE).is_ok_and(|value| value == "1")
}

/// The extensions of generated files. Other files, such as a Finder `.DS_Store`, are not compared.
const GENERATED_EXTENSIONS: &[&str] = &["json", "ts"];

/// The npm package of the JSON Schema and the TypeScript types.
const NPM_PACKAGE_NAME: &str = "@dyno-labs/schema";

fn is_generated_file(path: &Path) -> bool {
    let file_name = path.file_name().map(|name| name.to_string_lossy());
    let hidden = file_name.as_ref().is_some_and(|name| name.starts_with('.'));
    let generated_extension = path.extension().is_some_and(|extension| {
        GENERATED_EXTENSIONS
            .iter()
            .any(|generated| extension == *generated)
    });
    !hidden && generated_extension
}

/// Every generated file under `dir`, by path relative to `dir`, with its content.
fn read_tree(dir: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if is_generated_file(&path) {
                let relative = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let content = fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
                files.insert(relative, content);
            }
        }
    }
    files
}

/// Checks that `dir` holds exactly the generated files. When `DYNO_UPDATE_GENERATED=1` is set, rewrites `dir` instead.
///
/// A committed file that is no longer generated counts as stale too.
fn check_tree(dir: &Path, generated: &BTreeMap<String, String>) {
    let committed = read_tree(dir);
    if *generated == committed {
        return;
    }
    if updating_generated_files() {
        for name in committed
            .keys()
            .filter(|name| !generated.contains_key(*name))
        {
            fs::remove_file(dir.join(name)).unwrap();
        }
        for (name, content) in generated {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        return;
    }
    let changed = generated
        .keys()
        .filter(|name| generated.get(*name) != committed.get(*name));
    let orphaned = committed
        .keys()
        .filter(|name| !generated.contains_key(*name));
    let stale: Vec<&String> = changed.chain(orphaned).collect();
    panic!(
        "stale files in {}: {stale:?}. Run {UPDATE_GENERATED_VARIABLE}=1 INSTA_UPDATE=always cargo test --workspace",
        dir.display()
    );
}

#[test]
fn json_schemas_and_catalogues_are_current() {
    let generated: BTreeMap<String, String> = generate::json_files()
        .into_iter()
        .map(|(name, content)| (name.to_owned(), content))
        .collect();
    check_tree(&package_dir().join("json"), &generated);
}

#[test]
fn typescript_types_are_current() {
    let generated_dir = tempfile::tempdir().unwrap();
    generate::export_typescript(generated_dir.path()).unwrap();
    check_tree(&package_dir().join("ts"), &read_tree(generated_dir.path()));
}

fn npm_package() -> serde_json::Value {
    let text = fs::read_to_string(package_dir().join("package.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn the_types_package_has_the_workspace_version() {
    let package = npm_package();
    assert_eq!(package["name"], NPM_PACKAGE_NAME);
    assert_eq!(package["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn the_types_package_has_the_workspace_licence() {
    assert_eq!(npm_package()["license"], env!("CARGO_PKG_LICENSE"));
}

#[test]
fn the_types_package_points_at_the_generated_index() {
    let package = npm_package();
    assert_eq!(
        package["types"],
        format!("./ts/{}", generate::TYPESCRIPT_INDEX)
    );
    let index =
        fs::read_to_string(package_dir().join("ts").join(generate::TYPESCRIPT_INDEX)).unwrap();
    assert!(index.contains("export type * from \"./CheckResult\";"));
}

#[test]
fn the_types_package_ships_the_json_schemas() {
    let package = npm_package();
    assert_eq!(
        package["files"],
        serde_json::json!(["json/*.json", "ts/*.ts"])
    );
    assert_eq!(package["exports"]["./json/*"], "./json/*");
}
