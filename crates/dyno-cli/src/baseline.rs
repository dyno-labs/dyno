//! `--baseline`: the empty build inside dyno, one page with nothing on it. A device run of it
//! gives the phone's footprint with no game, which a game's footprint can be measured over.

use std::path::Path;

use crate::exit::CliError;

/// The empty build's one file, `index.html`. `tools/phone/run.sh` wrote the same bytes for its
/// baseline runs, so they have the same build id.
pub const EMPTY_PAGE: &str = "<!doctype html><title>baseline</title>\n";

/// The empty build, written to a folder that is deleted when it is dropped.
pub struct EmptyBuild {
    folder: tempfile::TempDir,
}

impl EmptyBuild {
    /// Writes the empty build to a new folder.
    pub fn write() -> Result<EmptyBuild, CliError> {
        let folder = tempfile::Builder::new()
            .prefix("dyno-baseline-")
            .tempdir()
            .map_err(CliError::internal)?;
        std::fs::write(folder.path().join("index.html"), EMPTY_PAGE).map_err(CliError::internal)?;
        Ok(EmptyBuild { folder })
    }

    /// The build folder.
    pub fn path(&self) -> &Path {
        self.folder.path()
    }
}

#[cfg(test)]
mod tests {
    use dyno_schema::profile::HostProfile;

    use super::*;

    /// The empty build's id. It never changes, because its one file never does. `docs/cli.md`
    /// gives it too.
    const EMPTY_BUILD_ID: &str = "7114691fda7748ccc7c091ffeae74100f190d0557000f8739823a040446c5c4e";

    #[test]
    fn the_empty_build_has_one_page_and_a_constant_id() {
        let build = EmptyBuild::write().unwrap();
        let scan = dyno_static::scan(build.path(), &HostProfile::reels(), None).unwrap();
        let manifest = scan.manifest.unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path.as_str(), "index.html");
        assert_eq!(manifest.build_id.as_deref(), Some(EMPTY_BUILD_ID));
    }

    #[test]
    fn the_folder_is_deleted_with_the_build() {
        let build = EmptyBuild::write().unwrap();
        let folder = build.path().to_owned();
        drop(build);
        assert!(!folder.exists());
    }
}
