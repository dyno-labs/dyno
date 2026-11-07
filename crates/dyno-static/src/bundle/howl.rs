//! Howl sites: Howler options objects with a literal `src`, and their sprite ranges (design
//! sections 6.1 and 8.7).

use dyno_schema::path::RelPath;

use crate::bundle::eval::{Evaluator, ObjectKeys, Property, Value};
use crate::model::{HowlSite, SpriteRange};

/// Every option a `Howl` takes, core and spatial, from Howler's documentation. The default
/// `howler` build has the spatial plugin. An object with any other key is not a Howl site, so
/// `{alias, src}` for Pixi's asset loader is not one.
const HOWL_OPTIONS: &[&str] = &[
    "autoplay",
    "format",
    "html5",
    "loop",
    "mute",
    "onend",
    "onfade",
    "onload",
    "onloaderror",
    "onmute",
    "onorientation",
    "onpause",
    "onplay",
    "onplayerror",
    "onpos",
    "onrate",
    "onseek",
    "onstereo",
    "onstop",
    "onunlock",
    "onvolume",
    "orientation",
    "pannerAttr",
    "pool",
    "pos",
    "preload",
    "rate",
    "sprite",
    "src",
    "stereo",
    "volume",
    "xhr",
    // Howler 2.0 took this before `xhr`.
    "xhrWithCredentials",
];

const SRC: &str = "src";
const SPRITE: &str = "sprite";

const MICROSECONDS_PER_MILLISECOND: f64 = 1_000.0;

/// The Howl site whose options object has `keys`, passed to a `new` that starts at byte
/// `offset` of `file`.
///
/// None when the object has a key Howl does not take, a computed key or a spread that was not
/// read, when `src` is not static, or when a sprite is not static. Audio reads such sprite maps
/// from JSON instead.
pub fn read_site<'a>(
    file: &RelPath,
    offset: u32,
    keys: &ObjectKeys<'_, 'a>,
    evaluator: &Evaluator<'_, 'a>,
) -> Option<HowlSite> {
    let every_name_is_an_option = keys.names().all(|name| HOWL_OPTIONS.contains(&name));
    if keys.may_have_other_keys() || !every_name_is_an_option {
        return None;
    }
    let Property::Written(src) = keys.get(SRC) else {
        return None;
    };
    let src_list = as_src_list(evaluator.value(src)?)?;
    let sprites = match keys.get(SPRITE) {
        Property::Absent => Vec::new(),
        Property::Hidden => return None,
        Property::Written(sprite) => match evaluator.object(sprite) {
            Some(sprite_object) => sprite_ranges(&evaluator.keys(sprite_object), evaluator)?,
            // Howler reads `sprite || {}`, so a falsy value is no sprites.
            None if !evaluator.value(sprite)?.is_truthy() => Vec::new(),
            None => return None,
        },
    };
    Some(HowlSite {
        file: file.clone(),
        offset,
        src: src_list,
        sprites,
    })
}

/// `src` as a list: one string, or an array of strings.
fn as_src_list(value: Value) -> Option<Vec<String>> {
    let src_list = match value {
        Value::Text(src) => vec![src],
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::Text(src) => Some(src),
                _ => None,
            })
            .collect::<Option<Vec<String>>>()?,
        _ => return None,
    };
    (!src_list.is_empty()).then_some(src_list)
}

/// Every sprite of a `sprite` object, sorted by name. None when any one is not static.
fn sprite_ranges<'a>(
    sprites: &ObjectKeys<'_, 'a>,
    evaluator: &Evaluator<'_, 'a>,
) -> Option<Vec<SpriteRange>> {
    if sprites.may_have_other_keys() {
        return None;
    }
    sprites
        .names()
        .map(|name| {
            let Property::Written(range) = sprites.get(name) else {
                return None;
            };
            sprite_range(name, evaluator.value(range)?)
        })
        .collect()
}

fn sprite_range(name: &str, range: Value) -> Option<SpriteRange> {
    let Value::Array(parts) = range else {
        return None;
    };
    let (start, duration, looped) = match parts.as_slice() {
        [start, duration] => (start, duration, false),
        [start, duration, looped] => (start, duration, looped.is_truthy()),
        _ => return None,
    };
    let start_ms = milliseconds(start)?;
    let duration_ms = milliseconds(duration)?;
    Some(SpriteRange {
        name: name.to_owned(),
        start_us: microseconds(start_ms),
        end_us: microseconds(start_ms + duration_ms),
        looped,
    })
}

/// A finite, non-negative number of milliseconds.
fn milliseconds(value: &Value) -> Option<f64> {
    value
        .as_number()
        .filter(|number| number.is_finite() && *number >= 0.0)
}

/// Rounded to whole microseconds. A huge value saturates.
fn microseconds(ms: f64) -> u64 {
    (ms * MICROSECONDS_PER_MILLISECOND).round() as u64
}

#[cfg(test)]
mod tests {
    use dyno_schema::profile::HostProfile;

    use super::*;
    use crate::bundle::js::{ScriptAt, scan_script};

    fn sites(text: &str) -> Vec<HowlSite> {
        let file = RelPath::new("main.js").unwrap();
        let at = ScriptAt {
            file: &file,
            offset: 0,
        };
        scan_script(at, text, &HostProfile::reels())
            .unwrap()
            .howl_sites
    }

    #[test]
    fn a_single_string_src_is_a_list_of_one() {
        let sites = sites("new Howl({src: 'a.mp3', volume: .5})");
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].src, ["a.mp3"]);
        assert_eq!(sites[0].sprites, []);
    }

    #[test]
    fn the_offset_is_the_call() {
        assert_eq!(sites("x(); new k.Howl({src: 'a.mp3'})")[0].offset, 5);
    }

    #[test]
    fn an_object_with_a_key_howl_does_not_take_is_not_a_site() {
        assert_eq!(sites("new Howl({alias: 'bunny', src: 'bunny.png'})"), []);
    }

    #[test]
    fn a_spatial_option_is_a_howl_option() {
        assert_eq!(sites("new Howl({src: 'a.mp3', stereo: -1})").len(), 1);
    }

    #[test]
    fn a_src_that_is_not_static_is_not_a_site() {
        assert_eq!(sites("new Howl({src: map.src, sprite: map.sprite})"), []);
    }

    #[test]
    fn an_empty_src_list_is_not_a_site() {
        assert_eq!(sites("new Howl({src: []})"), []);
    }

    #[test]
    fn an_object_with_a_computed_key_is_not_a_site() {
        assert_eq!(sites("new Howl({[k]: 1, src: 'a.mp3'})"), []);
    }

    #[test]
    fn an_object_with_a_spread_that_cannot_be_read_is_not_a_site() {
        assert_eq!(sites("new Howl({...o, src: 'a.mp3'})"), []);
        assert_eq!(
            sites("new Howl({...o, src: 'a.mp3', sprite: {a: [0, 1]}})"),
            []
        );
    }

    #[test]
    fn a_call_without_new_is_not_a_site() {
        assert_eq!(sites("Assets.load({src: 'bg.mp3'})"), []);
    }

    #[test]
    fn a_howler_2_0_option_is_a_howl_option() {
        let source = "new Howl({src: 'a.mp3', xhrWithCredentials: !0})";
        assert_eq!(sites(source).len(), 1);
    }

    #[test]
    fn a_sprite_with_a_value_that_is_not_static_is_not_a_site() {
        assert_eq!(sites("new Howl({src: 'a.mp3', sprite: {a: [0, d]}})"), []);
    }

    #[test]
    fn a_sprite_map_with_a_spread_that_cannot_be_read_is_not_a_site() {
        assert_eq!(sites("new Howl({src: 'a.mp3', sprite: {...more}})"), []);
    }

    #[test]
    fn a_falsy_sprite_is_a_site_with_no_sprites() {
        let found = sites("new Howl({src: 'a.mp3', sprite: null})");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sprites, []);
    }

    #[test]
    fn a_sprite_that_is_too_short_is_not_a_site() {
        assert_eq!(sites("new Howl({src: 'a.mp3', sprite: {a: [0]}})"), []);
    }

    #[test]
    fn a_sprite_with_a_negative_start_is_not_a_site() {
        assert_eq!(sites("new Howl({src: 'a.mp3', sprite: {a: [-5, 10]}})"), []);
    }

    #[test]
    fn a_truthy_third_sprite_element_loops() {
        let sites = sites("new Howl({src: 'a.mp3', sprite: {a: [0, 1, !0]}})");
        assert!(sites[0].sprites[0].looped);
    }

    #[test]
    fn a_falsy_third_sprite_element_does_not_loop() {
        let sites = sites("new Howl({src: 'a.mp3', sprite: {a: [0, 1, !1]}})");
        assert!(!sites[0].sprites[0].looped);
    }

    #[test]
    fn a_sprite_object_bound_to_a_variable_is_read_through_it() {
        let sites = sites("const s = {a: [250, 500]}; new Howl({src: ['a.webm'], sprite: s})");
        assert_eq!(
            sites[0].sprites,
            [SpriteRange {
                name: "a".into(),
                start_us: 250_000,
                end_us: 750_000,
                looped: false,
            }]
        );
    }
}
