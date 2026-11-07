//! Every fixture loads: its build, its profile and its inventory.

mod common;

use common::{fixtures, integer};
use toml::Value;

#[test]
fn every_build_holds_the_files_its_inventory_counts() {
    for fixture in fixtures() {
        let inventory = fixture.required_table("inventory");
        let files = fixture.files();
        let total_bytes: usize = files.values().map(Vec::len).sum();
        assert_eq!(
            files.len() as u64,
            integer(inventory, "file_count"),
            "{}",
            fixture.name
        );
        assert_eq!(
            total_bytes as u64,
            integer(inventory, "total_bytes"),
            "{}",
            fixture.name
        );
    }
}

#[test]
fn every_profile_table_gives_a_host_profile_with_its_values() {
    for fixture in fixtures() {
        let table = fixture.required_table("profile");
        let profile = fixture.profile();
        let name = &fixture.name;
        assert_eq!(
            u64::from(profile.device_scale_factor) * 1_000,
            integer(table, "dpr_milli"),
            "{name}"
        );
        assert_eq!(
            Value::from(vec![profile.viewport.width, profile.viewport.height]),
            table["frame_css"],
            "{name}"
        );
        assert_eq!(Value::from(profile.webgpu), table["webgpu"], "{name}");
    }
}

#[test]
fn every_folder_fixture_gives_the_inventory_it_lists() {
    for fixture in fixtures().iter().filter(|fixture| !fixture.is_zip()) {
        let inventory = fixture.required_table("inventory");
        let listed = fixture.inventory();
        assert_eq!(
            listed.files().len() as u64,
            integer(inventory, "file_count"),
            "{}",
            fixture.name
        );
        for file in listed.files() {
            let contents = listed
                .contents(&file.path)
                .expect("contents for every file");
            assert_eq!(
                contents.len() as u64,
                file.bytes,
                "{} {}",
                fixture.name,
                file.path
            );
        }
    }
}
