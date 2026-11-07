//! Duplicates: files that repeat another file byte for byte.
//!
//! It runs after textures and audio, so a group can say how many decoded bytes its copies cost
//! (design section 4). A group keeps the one copy the build names, so a fix never deletes the
//! copy the game loads.

use std::borrow::Cow;
use std::collections::BTreeMap;

use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::{FileKind, ManifestFile};
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, Uncertainty,
};
use dyno_schema::path::RelPath;

use crate::model::{DuplicateGroup, FamilyScan, Inventory, saturating_i64};
use crate::sniff::is_source_map;

/// The measures this family owns.
pub const MEASURES: &[MeasureId] = &[MeasureId::StaticBytesDuplicates];

/// Groups the manifest's files by SHA-256. `decoded_bytes` gives each texture's GPU bytes and each
/// audio file's bytes at 48 kHz, by path.
pub fn scan(
    inventory: &Inventory,
    decoded_bytes: &BTreeMap<RelPath, u64>,
) -> FamilyScan<Vec<DuplicateGroup>> {
    let groups = duplicate_groups(inventory, decoded_bytes);
    FamilyScan {
        measures: vec![wasted_bytes(&groups)],
        summary: groups,
    }
}

/// The kinds of file that can name another file by its path.
const NAMING_KINDS: [FileKind; 4] = [FileKind::Html, FileKind::Js, FileKind::Css, FileKind::Data];

/// Every set of two or more non-empty files with the same SHA-256, sorted by first path. Each copy
/// but one wastes one copy's bytes, and its decoded bytes when the file decodes.
pub fn duplicate_groups(
    inventory: &Inventory,
    decoded_bytes: &BTreeMap<RelPath, u64>,
) -> Vec<DuplicateGroup> {
    let texts = naming_texts(inventory);
    let all_paths: Vec<&RelPath> = inventory.files().iter().map(|file| &file.path).collect();
    let mut files_by_hash: BTreeMap<&str, Vec<&ManifestFile>> = BTreeMap::new();
    for file in inventory.files().iter().filter(|file| file.bytes > 0) {
        files_by_hash.entry(&file.sha256).or_default().push(file);
    }
    let mut groups: Vec<DuplicateGroup> = files_by_hash
        .into_values()
        .filter(|files| files.len() >= 2)
        .map(|files| {
            // The manifest is sorted by path, so the files are too.
            let first = files[0];
            let extra_copies = files.len() as u64 - 1;
            let decoded = decoded_bytes.get(&first.path).copied().unwrap_or(0);
            let paths: Vec<RelPath> = files.iter().map(|file| file.path.clone()).collect();
            DuplicateGroup {
                sha256: first.sha256.clone(),
                bytes: first.bytes,
                keep: only_named_copy(&paths, &all_paths, &texts),
                paths,
                wasted_bytes: extra_copies * first.bytes,
                decoded_bytes_saved: extra_copies * decoded,
            }
        })
        .collect();
    groups.sort_by(|left, right| left.paths[0].cmp(&right.paths[0]));
    groups
}

/// The copies a fix would delete: every copy but the one to keep, or but the first when the
/// build does not show which copy to keep.
pub fn extra_copies(group: &DuplicateGroup) -> Vec<&RelPath> {
    let kept = group.keep.as_ref().unwrap_or(&group.paths[0]);
    group.paths.iter().filter(|path| *path != kept).collect()
}

/// The text of every file that can name another, source maps left out. Invalid UTF-8 is replaced
/// with U+FFFD, so it never matches a path.
fn naming_texts(inventory: &Inventory) -> Vec<Cow<'_, str>> {
    NAMING_KINDS
        .iter()
        .flat_map(|&kind| inventory.files_of_kind(kind))
        .filter(|file| !is_source_map(&file.path))
        .filter_map(|file| inventory.contents(&file.path))
        .map(String::from_utf8_lossy)
        .collect()
}

/// The one copy that some text names, or None when no copy or more than one is named.
fn only_named_copy(
    copies: &[RelPath],
    all_paths: &[&RelPath],
    texts: &[Cow<'_, str>],
) -> Option<RelPath> {
    let named: Vec<&RelPath> = copies
        .iter()
        .filter(|copy| {
            let suffix = shortest_unique_suffix(copy, all_paths);
            texts.iter().any(|text| names_path(text, suffix))
        })
        .collect();
    match named.as_slice() {
        [only] => Some((*only).clone()),
        _ => None,
    }
}

/// The fewest trailing parts of `path` that no other file in the build ends with: `coin.png`
/// when it is the only one, `a/coin.png` when `c/coin.png` is in the build too.
fn shortest_unique_suffix<'p>(path: &'p RelPath, all_paths: &[&RelPath]) -> &'p str {
    let full = path.as_str();
    let starts = full
        .char_indices()
        .filter(|&(_, character)| character == '/')
        .map(|(index, _)| index + 1);
    let mut suffix_starts: Vec<usize> = starts.collect();
    suffix_starts.reverse();
    suffix_starts.push(0);
    for start in suffix_starts {
        let suffix = &full[start..];
        let is_unique = all_paths
            .iter()
            .filter(|other| other.as_str() != full)
            .all(|other| !ends_with_parts(other.as_str(), suffix));
        if is_unique {
            return suffix;
        }
    }
    full
}

/// Whether `path` ends with the whole parts of `suffix`: `b/a/coin.png` ends with `a/coin.png`,
/// but `bigcoin.png` does not end with `coin.png`.
fn ends_with_parts(path: &str, suffix: &str) -> bool {
    path == suffix || path.ends_with(&format!("/{suffix}"))
}

/// Whether `text` holds `suffix` as a whole name: the characters on each side are not part of a
/// file name, so `coin.png` does not match inside `bigcoin.png` or `coin.png2`.
fn names_path(text: &str, suffix: &str) -> bool {
    text.match_indices(suffix).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + suffix.len()..].chars().next();
        !before.is_some_and(is_name_character) && !after.is_some_and(is_name_character)
    })
}

fn is_name_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '-' | '_' | '.')
}

/// The sum of wasted bytes, with one item per copy a fix would delete.
fn wasted_bytes(groups: &[DuplicateGroup]) -> Measure {
    let items = groups
        .iter()
        .flat_map(|group| {
            extra_copies(group).into_iter().map(|path| MeasureItem {
                key: path.as_str().to_owned(),
                value: MeasureValue::Integer(saturating_i64(group.bytes)),
            })
        })
        .collect();
    Measure {
        value: Some(MeasureValue::Integer(saturating_i64(
            groups.iter().map(|group| group.wasted_bytes).sum::<u64>(),
        ))),
        items,
        evidence: vec![Evidence {
            method: Method::FileHash,
            detail:
                "files with the same SHA-256; the copy the build names is kept, else the first \
                     path"
                    .to_owned(),
        }],
        uncertainty: Uncertainty::exact(),
        ..Measure::blank(MeasureId::StaticBytesDuplicates, MeasureStatus::Estimated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(raw: &str) -> RelPath {
        RelPath::new(raw).unwrap()
    }

    #[test]
    fn a_unique_file_name_is_its_own_suffix() {
        let coin = path("a/coin.png");
        let gem = path("b/gem.png");
        assert_eq!(shortest_unique_suffix(&coin, &[&coin, &gem]), "coin.png");
    }

    #[test]
    fn a_shared_file_name_takes_its_folder_too() {
        let a = path("x/a/coin.png");
        let c = path("x/c/coin.png");
        assert_eq!(shortest_unique_suffix(&a, &[&a, &c]), "a/coin.png");
        let deep = path("y/a/coin.png");
        assert_eq!(shortest_unique_suffix(&a, &[&a, &deep]), "x/a/coin.png");
    }

    #[test]
    fn a_name_matches_only_whole() {
        assert!(names_path("src=\"audio/click.mp3\"", "click.mp3"));
        assert!(names_path("load(`./click.mp3`)", "click.mp3"));
        assert!(!names_path("load('big-click.mp3')", "click.mp3"));
        assert!(!names_path("load('click.mp3.bak')", "click.mp3"));
        assert!(!names_path("load('click.mp3x')", "click.mp3"));
    }

    #[test]
    fn only_one_named_copy_is_kept() {
        let old = path("audio/click-old.mp3");
        let new = path("audio/click.mp3");
        let copies = [old.clone(), new.clone()];
        let all = [&old, &new];
        let page = Cow::Borrowed("<audio src='audio/click.mp3'>");
        assert_eq!(only_named_copy(&copies, &all, &[page]), Some(new.clone()));
        let both = Cow::Borrowed("['click.mp3', 'click-old.mp3']");
        assert_eq!(only_named_copy(&copies, &all, &[both]), None);
        assert_eq!(only_named_copy(&copies, &all, &[]), None);
    }
}
