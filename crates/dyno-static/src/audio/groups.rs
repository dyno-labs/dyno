//! Sound groups: a game often ships one sound in several formats, and the browser plays one of
//! them, so a sound counts once (design section 1).
//!
//! A group joins the files in one folder with one stem (`sfx.webm`, `sfx.mp3`), and the files of
//! one Howler `src` list. Only files whose header was read join a group.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use dyno_schema::path::RelPath;

use super::{AudioFile, Decoded};

/// One sound, in one or more files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundGroup {
    /// Sorted.
    pub paths: Vec<RelPath>,
    /// The member with the most decoded bytes at 48 kHz. On a tie, the first path.
    pub representative: RelPath,
    /// The representative's.
    pub channels: u32,
    /// The representative's.
    pub decoded_bytes_48k: u64,
    /// The representative's.
    pub decoded_bytes_24k: u64,
}

/// Union-find over indices. A set's root is its smallest index, so its first path.
struct Partition {
    parent: Vec<usize>,
}

impl SoundGroup {
    /// Two or more channels.
    pub fn is_stereo(&self) -> bool {
        self.channels >= 2
    }

    /// What exporting the sound as mono saves at 48 kHz: every channel but one.
    pub fn mono_saving_48k(&self) -> u64 {
        self.decoded_bytes_48k - self.decoded_bytes_48k / u64::from(self.channels.max(1))
    }
}

/// The sound groups of `files`, sorted by representative. Each `src_lists` entry is one Howler
/// `src` list, resolved to build paths.
pub(super) fn sound_groups(files: &[AudioFile], src_lists: &[Vec<RelPath>]) -> Vec<SoundGroup> {
    let estimated: Vec<(&RelPath, &Decoded)> = files
        .iter()
        .filter_map(|file| Some((&file.path, file.decoded()?)))
        .collect();
    let mut partition = Partition::new(estimated.len());

    let mut first_with_stem: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (index, (path, _)) in estimated.iter().enumerate() {
        let first = *first_with_stem
            .entry(folder_and_stem(path))
            .or_insert(index);
        partition.join(first, index);
    }

    let index_of: BTreeMap<&RelPath, usize> = estimated
        .iter()
        .enumerate()
        .map(|(index, (path, _))| (*path, index))
        .collect();
    for src_list in src_lists {
        let members: Vec<usize> = src_list
            .iter()
            .filter_map(|path| index_of.get(path).copied())
            .collect();
        for pair in members.windows(2) {
            partition.join(pair[0], pair[1]);
        }
    }

    let mut members_by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for index in 0..estimated.len() {
        members_by_root
            .entry(partition.find(index))
            .or_default()
            .push(index);
    }
    let mut groups: Vec<SoundGroup> = members_by_root
        .values()
        .map(|members| {
            let representative = members
                .iter()
                .map(|&index| estimated[index])
                .min_by_key(|(path, decoded)| (Reverse(decoded.decoded_bytes_48k), *path))
                .expect("a group has a member");
            let (path, decoded) = representative;
            SoundGroup {
                paths: members
                    .iter()
                    .map(|&index| estimated[index].0.clone())
                    .collect(),
                representative: path.clone(),
                channels: decoded.header.channels,
                decoded_bytes_48k: decoded.decoded_bytes_48k,
                decoded_bytes_24k: decoded.decoded_bytes_24k,
            }
        })
        .collect();
    groups.sort_by(|a, b| a.representative.cmp(&b.representative));
    groups
}

/// A path's folder and its file name without the last extension.
fn folder_and_stem(path: &RelPath) -> (&str, &str) {
    let (folder, name) = path
        .as_str()
        .rsplit_once('/')
        .unwrap_or(("", path.as_str()));
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    (folder, stem)
}

impl Partition {
    fn new(size: usize) -> Partition {
        Partition {
            parent: (0..size).collect(),
        }
    }

    fn find(&mut self, mut index: usize) -> usize {
        while self.parent[index] != index {
            self.parent[index] = self.parent[self.parent[index]];
            index = self.parent[index];
        }
        index
    }

    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        self.parent[a.max(b)] = a.min(b);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Codec, Container, Header};
    use super::*;

    fn path(raw: &str) -> RelPath {
        RelPath::new(raw).unwrap()
    }

    /// An estimated file with this many decoded bytes at 48 kHz and channels.
    fn file(raw: &str, decoded_bytes_48k: u64, channels: u32) -> AudioFile {
        let header = Header {
            container: Container::Wav,
            codec: Codec::Pcm,
            channels,
            sample_rate: 48_000,
            ticks: decoded_bytes_48k / 4 / u64::from(channels),
            tick_rate: 48_000,
            note: None,
        };
        AudioFile {
            path: path(raw),
            reading: Ok(Decoded {
                header,
                decoded_bytes_48k,
                decoded_bytes_24k: decoded_bytes_48k / 2,
            }),
        }
    }

    fn paths_of(groups: &[SoundGroup]) -> Vec<Vec<&str>> {
        groups
            .iter()
            .map(|group| group.paths.iter().map(RelPath::as_str).collect())
            .collect()
    }

    #[test]
    fn files_with_one_folder_and_stem_form_one_group() {
        let files = [
            file("a/sfx.mp3", 8, 1),
            file("a/sfx.webm", 8, 1),
            file("b/sfx.mp3", 8, 1),
        ];
        let groups = sound_groups(&files, &[]);
        assert_eq!(
            paths_of(&groups),
            [vec!["a/sfx.mp3", "a/sfx.webm"], vec!["b/sfx.mp3"]]
        );
    }

    #[test]
    fn a_src_list_joins_files_with_other_stems() {
        let files = [
            file("a/hit.mp3", 8, 1),
            file("b/hit-hq.ogg", 8, 1),
            file("c.wav", 8, 1),
        ];
        let src_list = vec![path("b/hit-hq.ogg"), path("missing.mp3"), path("a/hit.mp3")];
        let groups = sound_groups(&files, &[src_list]);
        assert_eq!(
            paths_of(&groups),
            [vec!["a/hit.mp3", "b/hit-hq.ogg"], vec!["c.wav"]]
        );
    }

    #[test]
    fn the_representative_decodes_largest_and_ties_go_to_the_first_path() {
        let largest = [
            file("s.mp3", 8, 1),
            file("s.ogg", 16, 2),
            file("s.wav", 8, 1),
        ];
        let tied = [file("t.ogg", 8, 1), file("t.mp3", 8, 1)];
        assert_eq!(sound_groups(&largest, &[])[0].representative, path("s.ogg"));
        assert_eq!(sound_groups(&tied, &[])[0].representative, path("t.mp3"));
    }

    #[test]
    fn mono_saves_every_channel_but_one() {
        let stereo = &sound_groups(&[file("s.wav", 800, 2)], &[])[0];
        let mono = &sound_groups(&[file("m.wav", 800, 1)], &[])[0];
        assert!(stereo.is_stereo() && !mono.is_stereo());
        assert_eq!((stereo.mono_saving_48k(), mono.mono_saving_48k()), (400, 0));
    }
}
