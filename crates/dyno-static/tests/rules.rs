//! The Phase 1 static rules, checked against the summaries that each fixture's tables describe
//! (design section 9).

mod common;

use std::path::Path;

use common::{Fixture, fixtures, integer, path, paths, rows, sub_table, text};
use dyno_schema::SCHEMA_VERSION;
use dyno_schema::budget::{BYTES_PER_MB, Budget};
use dyno_schema::config::ConfigFormat;
use dyno_schema::ids::{MeasureId, RuleId};
use dyno_schema::measure::{MeasureStatus, NOT_BUILT_YET};
use dyno_schema::path::RelPath;
use dyno_schema::profile::{HostProfile, Viewport};
use dyno_schema::rule::FixAction;
use dyno_schema::status::Verdict;
use dyno_static::NEEDS_A_BUILD;
use dyno_static::audio::StereoClass;
use dyno_static::model::{
    AudioSummary, BundleSummary, CanvasFloor, DecodedAudio, DuplicateGroup, MeasureWithoutValue,
    RenderSite, Resolution, SpriteMapSummary, StereoSound, Summaries, TextureCost, TextureSummary,
    canvas_floor,
};
use dyno_static::rules::{
    self, DEFAULT_BUDGET, NO_CANVAS, NO_RENDER_CONFIG, RuleReport, RuleValue,
};
use toml::{Table, Value};

/// What a `[rules."<id>"]` table says about one rule. An optional key the table leaves out is None
/// on both sides, so it is not compared.
#[derive(Debug, PartialEq, Eq)]
struct RuleRow {
    outcome: Verdict,
    value: Option<RuleValue>,
    saved_bytes: u64,
    saved_decoded_bytes: Option<u64>,
    items: Option<Vec<RelPath>>,
    reason: Option<String>,
}

const MB: u64 = BYTES_PER_MB;

#[test]
fn every_fixture_gives_the_rule_outcomes_it_lists() {
    let mut mismatches = Vec::new();
    for fixture in fixtures() {
        let reports = rules::reports(&summaries(&fixture), None);
        let listed_rules = fixture.required_table("rules");
        for report in &reports {
            let rule = report.finding.rule;
            if rule == RuleId::FilesUnreferenced {
                continue;
            }
            let row = sub_table(listed_rules, rule.as_str());
            let (listed, actual) = (listed_row(row), actual_row(report, row));
            if listed != actual {
                mismatches.push(format!(
                    "{} {rule}\n  listed {listed:?}\n  actual {actual:?}",
                    fixture.name
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn each_fix_text_gives_its_sizes_and_paths() {
    let cases = [
        (
            "over-budget",
            RuleId::AudioPcmOverBudget,
            "Decoded audio is 131.8 MB at 48 kHz. Run the AudioContext at 24 kHz to save 65.9 MB, \
             or load long music when it is needed.",
        ),
        (
            "audio-formats",
            RuleId::AudioStereo,
            "4 stereo sounds decode to 1.5 MB. Export them as mono to save 0.7 MB. Mono would \
             change the sound of 3 of them, so listen first.",
        ),
        (
            "over-budget",
            RuleId::AudioStereo,
            "1 stereo sound decodes to 131.8 MB. Export it as mono to save 65.9 MB.",
        ),
        (
            "howler-sprites",
            RuleId::AudioSilenceShare,
            "62.5% of assets/audio/sfx.webm is not covered by any sprite in main.js. Trim the \
             gaps to save up to 1.4 MB.",
        ),
        (
            "pixi7-aa",
            RuleId::RenderAntialiasOrResolution,
            "main.js turns antialias on. Set `antialias: false`. A device run will measure what it \
             saves, so this fix has no number yet. main.js sets resolution 3. Cap `resolution` at \
             2 (`Math.min(devicePixelRatio, 2)`) to save 6.7 MB.",
        ),
        (
            "pixi7-opts",
            RuleId::RenderAntialiasOrResolution,
            "main.js sets resolution 3. Cap `resolution` at 2 (`Math.min(devicePixelRatio, 2)`) \
             to save 17.6 MB.",
        ),
        (
            "pixi7-opts",
            RuleId::TexturesOverBudget,
            "Textures decode to 3.5 to 7.0 MB. The largest: assets/bg-720x1280.webp. Ship it as \
             KTX2 (Basis) to save about 2.6 MB.",
        ),
        (
            "duplicates",
            RuleId::FilesDuplicate,
            "The build does not show which copy the game loads of a/coin.png, \
             audio/click-old.mp3, data/config.backup.json. Keep that copy and delete the others. \
             This saves 6.6 KB to download and up to 0.1 MB decoded.",
        ),
    ];
    for (name, rule, text) in cases {
        let fixture = common::fixture(name);
        let reports = rules::reports(&summaries(&fixture), None);
        assert_eq!(
            report_for(&reports, rule).text.as_deref(),
            Some(text),
            "{name} {rule}"
        );
    }
}

#[test]
fn a_rule_with_nothing_to_save_has_no_fix_and_no_text() {
    for rule in [RuleId::AudioPcmOverBudget, RuleId::FilesDuplicate] {
        let report = report(&Summaries::default(), None, rule);
        assert_eq!(report.finding.fixes, [], "{rule}");
        assert_eq!(report.text, None, "{rule}");
    }
}

#[test]
fn the_example_budget_file_holds_the_defaults() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/budget.toml");
    let example = Budget::load(&example).unwrap();
    assert_eq!(
        Budget {
            name: None,
            ..example
        },
        DEFAULT_BUDGET
    );
}

#[test]
fn a_limit_in_the_budget_file_replaces_the_default() {
    let summaries = decoded_audio(70 * MB);
    let rule = RuleId::AudioPcmOverBudget;
    assert_eq!(verdict(&summaries, None, rule), Verdict::Warn);
    let budget = budget("[audio]\ndecoded_mb = { warn = 100 }");
    assert_eq!(verdict(&summaries, Some(&budget), rule), Verdict::Pass);
}

#[test]
fn a_key_the_budget_file_leaves_out_keeps_its_default() {
    let budget = budget("[textures]\ndecoded_mb = { warn = 1 }");
    let summaries = decoded_audio(130 * MB);
    let rule = RuleId::AudioPcmOverBudget;
    assert_eq!(verdict(&summaries, Some(&budget), rule), Verdict::Fail);
}

#[test]
fn a_value_must_be_above_a_limit_to_reach_it() {
    let rule = RuleId::AudioPcmOverBudget;
    assert_eq!(verdict(&decoded_audio(64 * MB), None, rule), Verdict::Pass);
    assert_eq!(
        verdict(&decoded_audio(64 * MB + 1), None, rule),
        Verdict::Warn
    );
}

#[test]
fn a_lower_bound_above_a_limit_still_reaches_it() {
    let mut summaries = decoded_audio(70 * MB);
    summaries.audio.not_covered = vec![path("broken.ogg")];
    let rule = RuleId::AudioPcmOverBudget;
    assert_eq!(verdict(&summaries, None, rule), Verdict::Warn);
}

#[test]
fn a_lower_bound_below_every_limit_is_incomplete() {
    let mut summaries = decoded_audio(MB);
    summaries.audio.not_covered = vec![path("broken.ogg")];
    let report = report(&summaries, None, RuleId::AudioPcmOverBudget);
    assert_eq!(report.finding.verdict, Verdict::Incomplete);
    assert_eq!(
        report.finding.reason.as_deref(),
        Some("no estimate for 1 file: broken.ogg")
    );
}

#[test]
fn a_rule_that_would_pass_while_it_reads_a_measure_with_no_value_is_incomplete() {
    let summaries = Summaries {
        measures_without_value: vec![MeasureWithoutValue {
            id: MeasureId::StaticAudioDecoded48k,
            reason: NOT_BUILT_YET.to_owned(),
        }],
        ..Summaries::default()
    };
    let pcm = report(&summaries, None, RuleId::AudioPcmOverBudget);
    assert_eq!(pcm.finding.verdict, Verdict::Incomplete);
    assert_eq!(
        pcm.finding.reason.as_deref(),
        Some("static.audio.decoded_48k: not built yet")
    );
    let stereo = report(&summaries, None, RuleId::AudioStereo);
    assert_eq!(stereo.finding.verdict, Verdict::Pass);
}

#[test]
fn a_rule_that_warns_or_fails_keeps_its_verdict_when_a_measure_has_no_value() {
    let summaries = Summaries {
        measures_without_value: vec![MeasureWithoutValue {
            id: MeasureId::StaticAudioDecoded48k,
            reason: "2 files did not decode".to_owned(),
        }],
        ..decoded_audio(130 * MB)
    };
    assert_eq!(
        verdict(&summaries, None, RuleId::AudioPcmOverBudget),
        Verdict::Fail
    );
}

#[test]
fn an_incomplete_rule_keeps_its_own_reason_over_a_measure_with_no_value() {
    let summaries = Summaries {
        measures_without_value: vec![MeasureWithoutValue {
            id: MeasureId::StaticRenderAntialias,
            reason: "no render config found in the scripts".to_owned(),
        }],
        ..Summaries::default()
    };
    let render = report(&summaries, None, RuleId::RenderAntialiasOrResolution);
    assert_eq!(render.finding.reason.as_deref(), Some(NO_RENDER_CONFIG));
}

#[test]
fn files_unreferenced_is_not_built_yet() {
    let report = report(&Summaries::default(), None, RuleId::FilesUnreferenced);
    assert_eq!(report.finding.verdict, Verdict::Incomplete);
    assert_eq!(report.finding.reason.as_deref(), Some(NOT_BUILT_YET));
}

#[test]
fn without_a_build_every_rule_needs_one() {
    let findings = rules::without_build();
    assert_eq!(findings.len(), rules::RULES.len());
    for finding in findings {
        assert_eq!(finding.verdict, Verdict::Incomplete);
        assert_eq!(finding.reason.as_deref(), Some(NEEDS_A_BUILD));
    }
}

#[test]
fn a_duplicate_warns_by_default_and_allow_turns_the_check_off() {
    let summaries = duplicates(vec![duplicate(["a.png", "b.png"], 10, 0)]);
    let rule = RuleId::FilesDuplicate;
    assert_eq!(verdict(&summaries, None, rule), Verdict::Warn);
    let allow = budget("[files]\nduplicate = \"allow\"");
    assert_eq!(verdict(&summaries, Some(&allow), rule), Verdict::Pass);
}

#[test]
fn duplicate_groups_are_named_largest_waste_first() {
    let summaries = duplicates(vec![
        duplicate(["a.png", "b.png"], 10, 0),
        duplicate(["c.png", "d.png"], 20, 0),
    ]);
    let report = report(&summaries, None, RuleId::FilesDuplicate);
    assert_eq!(report.items, [path("c.png"), path("a.png")]);
}

#[test]
fn one_copy_reads_in_the_singular() {
    let summaries = duplicates(vec![duplicate(["a.png", "b.png"], 2_048, MB)]);
    let report = report(&summaries, None, RuleId::FilesDuplicate);
    assert_eq!(
        report.text.as_deref(),
        Some(
            "1 file is a copy of a file the game names: b.png. Delete it. This saves 2.0 KB to \
             download and up to 1.0 MB decoded."
        )
    );
}

#[test]
fn a_copy_the_build_names_is_kept_and_the_others_are_deleted() {
    let group = DuplicateGroup {
        keep: Some(path("b.png")),
        ..duplicate(["a.png", "b.png"], 10, 0)
    };
    let report = report(&duplicates(vec![group]), None, RuleId::FilesDuplicate);
    let deleted: Vec<_> = report
        .finding
        .fixes
        .iter()
        .map(|fix| fix.path.clone())
        .collect();
    assert_eq!(deleted, [Some(path("a.png"))]);
}

#[test]
fn a_group_whose_loaded_copy_is_not_known_gets_one_keep_fix() {
    let group = DuplicateGroup {
        keep: None,
        paths: vec![path("a.png"), path("b.png"), path("c.png")],
        wasted_bytes: 20,
        decoded_bytes_saved: 8 * MB,
        ..duplicate(["a.png", "b.png"], 10, 0)
    };
    let report = report(&duplicates(vec![group]), None, RuleId::FilesDuplicate);
    assert_eq!(fix_actions(&report), [FixAction::KeepOneCopy]);
    let fix = &report.finding.fixes[0];
    assert_eq!(fix.path, Some(path("a.png")));
    assert_eq!(
        (fix.saves_bytes, fix.saves_decoded_bytes),
        (20, Some(8 * MB))
    );
    assert_eq!(
        report.text.as_deref(),
        Some(
            "The build does not show which copy the game loads of a.png. Keep that copy and \
             delete the others. This saves 0.0 KB to download and up to 8.0 MB decoded."
        )
    );
}

#[test]
fn antialias_warns_by_default_with_no_fix_until_a_device_run() {
    let summaries = render_sites(vec![render_site("main.js", Some(true), Some(2_000))]);
    let report = report(&summaries, None, RuleId::RenderAntialiasOrResolution);
    assert_eq!(report.finding.verdict, Verdict::Warn);
    assert_eq!(fix_actions(&report), []);
    assert_eq!(report.items, [path("main.js")]);
    assert_eq!(
        report.text.as_deref(),
        Some(
            "main.js turns antialias on. Set `antialias: false`. A device run will measure what \
             it saves, so this fix has no number yet."
        )
    );
}

#[test]
fn an_uncapped_resolution_gets_a_fix_and_each_change_its_own_check() {
    let summaries = render_sites(vec![render_site("main.js", Some(true), Some(3_000))]);
    let rule = RuleId::RenderAntialiasOrResolution;
    let report = report(&summaries, None, rule);
    assert_eq!(fix_actions(&report), [FixAction::CapResolution]);
    let antialias_fails = budget("[render]\nantialias = \"fail\"");
    assert_eq!(
        verdict(&summaries, Some(&antialias_fails), rule),
        Verdict::Fail
    );
    let allow_antialias = budget("[render]\nantialias = \"allow\"");
    assert_eq!(
        verdict(&summaries, Some(&allow_antialias), rule),
        Verdict::Warn
    );
    let allow_both = budget("[render]\nantialias = \"allow\"\nresolution_uncapped = \"allow\"");
    assert_eq!(verdict(&summaries, Some(&allow_both), rule), Verdict::Pass);
}

#[test]
fn the_cap_saves_the_colour_buffer_pixels_above_resolution_2() {
    let site = render_site("main.js", Some(false), Some(3_000));
    let report = report(
        &render_sites(vec![site.clone()]),
        None,
        RuleId::RenderAntialiasOrResolution,
    );
    let at_3 = canvas_floor(site.canvas_css, 3_000).bytes;
    let at_2 = canvas_floor(site.canvas_css, 2_000).bytes;
    assert_eq!(report.saved_bytes(), at_3 - at_2);
}

#[test]
fn a_resolution_proven_above_the_cap_but_not_fixed_warns_with_no_fix() {
    let site = RenderSite {
        resolution: Resolution {
            low_milli: Some(2_500),
            high_milli: None,
        },
        ..render_site("main.js", Some(false), None)
    };
    let report = report(
        &render_sites(vec![site]),
        None,
        RuleId::RenderAntialiasOrResolution,
    );
    assert_eq!(report.finding.verdict, Verdict::Warn);
    assert_eq!(fix_actions(&report), []);
    assert_eq!(
        report.text.as_deref(),
        Some(
            "main.js sets a resolution above 2. Cap `resolution` at 2 (`Math.min(devicePixelRatio, 2)`)."
        )
    );
}

#[test]
fn the_rule_names_the_sites_that_warn_and_else_the_largest_floor() {
    let small = render_site("a.js", Some(false), Some(2_000));
    let large = render_site("b.js", Some(false), Some(3_000));
    let rule = RuleId::RenderAntialiasOrResolution;
    let worst = report(&render_sites(vec![small.clone(), large]), None, rule);
    assert_eq!(worst.items, [path("b.js")]);
    let twin = RenderSite {
        file: path("c.js"),
        ..small.clone()
    };
    let first = report(&render_sites(vec![small, twin]), None, rule);
    assert_eq!(first.finding.verdict, Verdict::Pass);
    assert_eq!(first.items, [path("a.js")]);
}

#[test]
fn render_options_set_at_run_time_leave_a_passing_rule_incomplete() {
    let site = render_site("main.js", None, Some(2_000));
    let report = report(
        &render_sites(vec![site]),
        None,
        RuleId::RenderAntialiasOrResolution,
    );
    assert_eq!(report.finding.verdict, Verdict::Incomplete);
    assert_eq!(
        report.finding.reason.as_deref(),
        Some("main.js sets antialias at run time")
    );
    let both = render_site("main.js", None, None);
    let report = report_of_render(vec![both]);
    assert_eq!(
        report.finding.reason.as_deref(),
        Some("main.js sets antialias and the resolution at run time")
    );
}

#[test]
fn a_proven_warning_wins_over_an_option_set_at_run_time() {
    let antialias_unknown = render_site("main.js", None, Some(3_000));
    assert_eq!(
        report_of_render(vec![antialias_unknown]).finding.verdict,
        Verdict::Warn
    );
    let resolution_unknown = render_site("a.js", Some(true), None);
    assert_eq!(
        report_of_render(vec![resolution_unknown]).finding.verdict,
        Verdict::Warn
    );
    let sites = vec![
        render_site("a.js", None, None),
        render_site("b.js", Some(true), Some(2_000)),
    ];
    let report = report_of_render(sites);
    assert_eq!(report.finding.verdict, Verdict::Warn);
    assert_eq!(report.items, [path("b.js")]);
}

#[test]
fn a_script_that_did_not_parse_leaves_a_passing_render_check_incomplete() {
    let mut summaries = render_sites(vec![render_site("main.js", Some(false), Some(2_000))]);
    summaries.bundle.invalid_scripts = vec![path("game.js")];
    let rule = RuleId::RenderAntialiasOrResolution;
    let report = report(&summaries, None, rule);
    assert_eq!(report.finding.verdict, Verdict::Incomplete);
    assert_eq!(
        report.finding.reason.as_deref(),
        Some("1 script did not parse and may hold a render config: game.js")
    );
    summaries.bundle.render_sites = vec![render_site("main.js", Some(true), Some(2_000))];
    assert_eq!(verdict(&summaries, None, rule), Verdict::Warn);
}

#[test]
fn antialias_on_a_2d_canvas_does_not_count() {
    let canvas_2d = RenderSite {
        draws_in_2d: true,
        ..render_site("a.js", None, Some(2_000))
    };
    assert_eq!(
        report_of_render(vec![canvas_2d.clone()]).finding.verdict,
        Verdict::Pass
    );
    let with_antialias_unsupported = Summaries {
        measures_without_value: vec![MeasureWithoutValue {
            id: MeasureId::StaticRenderAntialias,
            reason: "canvas2d".to_owned(),
        }],
        ..render_sites(vec![canvas_2d.clone()])
    };
    let rule = RuleId::RenderAntialiasOrResolution;
    assert_eq!(
        report(&with_antialias_unsupported, None, rule)
            .finding
            .verdict,
        Verdict::Pass
    );
    let on = RenderSite {
        antialias: Some(true),
        ..canvas_2d
    };
    assert_eq!(report_of_render(vec![on]).finding.verdict, Verdict::Pass);
}

#[test]
fn a_game_that_draws_with_html_passes_the_render_rule() {
    let no_canvas = |id| MeasureWithoutValue {
        id,
        reason: "no canvas: the game draws with HTML and CSS".to_owned(),
    };
    let summaries = Summaries {
        bundle: BundleSummary {
            draws_with_html: true,
            ..BundleSummary::default()
        },
        measures_without_value: vec![
            no_canvas(MeasureId::StaticRenderAntialias),
            no_canvas(MeasureId::StaticRenderResolutionUncapped),
        ],
        ..Summaries::default()
    };
    let report = report(&summaries, None, RuleId::RenderAntialiasOrResolution);
    assert_eq!(report.finding.verdict, Verdict::Pass);
    assert_eq!(report.finding.fixes, []);
}

#[test]
fn no_site_that_can_start_or_no_render_config_leave_the_rule_incomplete() {
    let cannot_start = RenderSite {
        canvas: None,
        ..render_site("main.js", Some(false), Some(3_000))
    };
    let rule = RuleId::RenderAntialiasOrResolution;
    let no_canvas = report(&render_sites(vec![cannot_start]), None, rule);
    assert_eq!(no_canvas.finding.reason.as_deref(), Some(NO_CANVAS));
    let none = report(&Summaries::default(), None, rule);
    assert_eq!(none.finding.reason.as_deref(), Some(NO_RENDER_CONFIG));
}

#[test]
fn a_texture_that_astc_would_not_shrink_gets_no_fix() {
    let texture = |name: &str, gpu_bytes, astc_bytes| TextureCost {
        path: path(name),
        gpu_bytes,
        astc_bytes,
    };
    let summaries = Summaries {
        textures: TextureSummary {
            files: vec![
                texture("a.ktx2", 2 * MB, 2 * MB),
                texture("b.png", MB, MB / 4),
            ],
            ..TextureSummary::default()
        },
        ..Summaries::default()
    };
    let report = report(&summaries, None, RuleId::TexturesOverBudget);
    assert_eq!(report.items, [path("b.png")]);
}

#[test]
fn a_stereo_sound_too_short_to_halve_gets_no_fix() {
    let sound = |name: &str, decoded_bytes_48k| StereoSound {
        path: path(name),
        decoded_bytes_48k,
        class: None,
    };
    let summaries = Summaries {
        audio: AudioSummary {
            stereo_groups: vec![sound("click.wav", 1), sound("music.ogg", 4 * MB)],
            ..AudioSummary::default()
        },
        ..Summaries::default()
    };
    let report = report(&summaries, None, RuleId::AudioStereo);
    assert_eq!(report.items, [path("music.ogg")]);
    assert_eq!(
        report.text.as_deref(),
        Some("1 stereo sound decodes to 4.0 MB. Export it as mono to save 2.0 MB.")
    );
}

#[test]
fn a_stereo_sound_that_mono_would_cancel_keeps_its_stereo() {
    let sound = |name: &str, class| StereoSound {
        path: path(name),
        decoded_bytes_48k: 4 * MB,
        class,
    };
    let summaries = Summaries {
        audio: AudioSummary {
            stereo_groups: vec![
                sound("ambience.ogg", Some(StereoClass::ChangesTheSound)),
                sound("music.ogg", Some(StereoClass::SafeToMakeMono)),
                sound("not-decoded.opus", None),
                sound("wide.ogg", Some(StereoClass::Cancels)),
            ],
            ..AudioSummary::default()
        },
        ..Summaries::default()
    };
    let report = report(&summaries, None, RuleId::AudioStereo);
    assert_eq!(
        report.items,
        [
            path("ambience.ogg"),
            path("music.ogg"),
            path("not-decoded.opus")
        ]
    );
    assert_eq!(report.value, Some(RuleValue::Bytes(6 * MB)));
    assert_eq!(
        report.text.as_deref(),
        Some(
            "3 stereo sounds decode to 12.0 MB. Export them as mono to save 6.0 MB. Mono would \
             change the sound of 1 of them, so listen first. Keep 1 other stereo sound as it is: \
             mono would cancel it out of phase."
        )
    );
}

#[test]
fn a_fix_text_asks_for_a_listen_when_mono_would_change_every_sound() {
    let sound = |name: &str| StereoSound {
        path: path(name),
        decoded_bytes_48k: 2 * MB,
        class: Some(StereoClass::ChangesTheSound),
    };
    let text = |sounds: Vec<StereoSound>| {
        let summaries = Summaries {
            audio: AudioSummary {
                stereo_groups: sounds,
                ..AudioSummary::default()
            },
            ..Summaries::default()
        };
        report(&summaries, None, RuleId::AudioStereo).text
    };
    assert_eq!(
        text(vec![sound("a.ogg")]).as_deref(),
        Some(
            "1 stereo sound decodes to 2.0 MB. Export it as mono to save 1.0 MB. Mono would \
             change its sound, so listen first."
        )
    );
    assert_eq!(
        text(vec![sound("a.ogg"), sound("b.ogg")]).as_deref(),
        Some(
            "2 stereo sounds decode to 4.0 MB. Export them as mono to save 2.0 MB. Mono would \
             change the sound of all 2, so listen first."
        )
    );
}

#[test]
fn a_sprite_map_must_be_above_both_silence_limits() {
    let summaries = sprite_maps(vec![
        sprite_map("long-gap-in-a-short-file.json", 9_000, MB / 2),
        sprite_map("short-gap-in-a-long-file.json", 500, 2 * MB),
        sprite_map("both.json", 2_000, 2 * MB),
    ]);
    let report = report(&summaries, None, RuleId::AudioSilenceShare);
    assert_eq!(report.finding.verdict, Verdict::Warn);
    assert_eq!(report.items, [path("both.json")]);
    assert_eq!(report.value, Some(RuleValue::BasisPoints(9_000)));
}

#[test]
fn a_silence_level_applies_only_when_both_keys_set_it() {
    let summaries = sprite_maps(vec![sprite_map("music.json", 9_000, 10 * MB)]);
    let share_fails = budget("[audio]\nsilence_share_bp = { warn = 1000, fail = 5000 }");
    let rule = RuleId::AudioSilenceShare;
    assert_eq!(verdict(&summaries, Some(&share_fails), rule), Verdict::Warn);
    let both_fail = budget(
        "[audio]\nsilence_share_bp = { warn = 1000, fail = 5000 }\n\
         silence_saving_mb = { warn = 1, fail = 5 }",
    );
    assert_eq!(verdict(&summaries, Some(&both_fail), rule), Verdict::Fail);
}

#[test]
fn a_sprite_map_with_no_estimate_or_a_script_that_did_not_parse_leaves_silence_incomplete() {
    let invalid_map = SpriteMapSummary {
        status: MeasureStatus::Invalid,
        ..sprite_map("bad.json", 9_000, 10 * MB)
    };
    let summaries = Summaries {
        bundle: BundleSummary {
            invalid_scripts: vec![path("game.js")],
            ..BundleSummary::default()
        },
        ..sprite_maps(vec![invalid_map])
    };
    let report = report(&summaries, None, RuleId::AudioSilenceShare);
    assert_eq!(report.finding.verdict, Verdict::Incomplete);
    assert_eq!(
        report.finding.reason.as_deref(),
        Some(
            "no estimate for 1 sprite map: bad.json; 1 script did not parse and may hold sprite \
             maps: game.js"
        )
    );
    assert_eq!(report.value, Some(RuleValue::BasisPoints(0)));
}

/// The summaries that the fixture's tables describe. A missing table means the build has
/// nothing of that kind.
fn summaries(fixture: &Fixture) -> Summaries {
    let duplicates = rows(fixture.required_table("duplicates"), "group");
    Summaries {
        bundle: fixture
            .table("bundle")
            .map(bundle_summary)
            .unwrap_or_default(),
        textures: fixture
            .table("textures")
            .map(texture_summary)
            .unwrap_or_default(),
        audio: fixture
            .table("audio")
            .map(|audio| audio_summary(audio, fixture.table("sprites")))
            .unwrap_or_default(),
        duplicates: duplicates.into_iter().map(duplicate_group).collect(),
        measures_without_value: Vec::new(),
    }
}

fn bundle_summary(bundle: &Table) -> BundleSummary {
    BundleSummary {
        render_sites: rows(bundle, "render_site")
            .into_iter()
            .map(listed_render_site)
            .collect(),
        invalid_scripts: paths(bundle, "invalid"),
        draws_with_html: bundle
            .get("draws_with_html")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..BundleSummary::default()
    }
}

fn listed_render_site(row: &Table) -> RenderSite {
    let [width, height] = pair(row, "canvas_css");
    let canvas = row.contains_key("canvas_bytes").then(|| {
        let [backing_width, backing_height] = pair(row, "canvas_backing");
        CanvasFloor {
            backing_width,
            backing_height,
            pixels: integer(row, "canvas_pixels"),
            bytes: integer(row, "canvas_bytes"),
        }
    });
    RenderSite {
        file: path(text(row, "file")),
        offset: 0,
        antialias: row.get("antialias").and_then(Value::as_bool),
        draws_in_2d: row.get("modelled_as").and_then(Value::as_str) == Some("canvas2d"),
        resolution: if row.contains_key("resolution_milli") {
            Resolution::exactly(small_integer(row, "resolution_milli"))
        } else {
            Resolution::default()
        },
        canvas_css: Viewport { width, height },
        canvas,
    }
}

fn texture_summary(textures: &Table) -> TextureSummary {
    let total = sub_table(textures, "total");
    let files = estimated_rows(textures, "file").map(|row| TextureCost {
        path: path(text(row, "path")),
        gpu_bytes: integer(row, "gpu_bytes"),
        astc_bytes: integer(row, "astc_bytes"),
    });
    TextureSummary {
        low_bytes: integer(total, "lo_bytes"),
        high_bytes: integer(total, "hi_bytes"),
        files: files.collect(),
        not_covered: paths(total, "not_covered"),
    }
}

fn audio_summary(audio: &Table, sprites: Option<&Table>) -> AudioSummary {
    let total = sub_table(audio, "total");
    let files = estimated_rows(audio, "file").map(|row| DecodedAudio {
        path: path(text(row, "path")),
        decoded_bytes_48k: integer(row, "decoded_bytes_48k"),
    });
    let mut stereo_groups: Vec<StereoSound> = rows(audio, "group")
        .into_iter()
        .filter(|group| integer(group, "channels") == 2)
        .map(|group| StereoSound {
            path: path(text(group, "representative")),
            decoded_bytes_48k: integer(group, "decoded_bytes_48k"),
            class: group
                .get("stereo_class")
                .and_then(Value::as_str)
                .map(stereo_class_named),
        })
        .collect();
    stereo_groups.sort_by(|left, right| left.path.cmp(&right.path));
    let sprite_rows = sprites.map(|sprites| rows(sprites, "map"));
    let mut sprite_maps: Vec<SpriteMapSummary> = sprite_rows
        .unwrap_or_default()
        .into_iter()
        .map(listed_sprite_map)
        .collect();
    sprite_maps.sort_by(|left, right| left.source.cmp(&right.source));
    AudioSummary {
        decoded_bytes_48k: integer(total, "decoded_bytes_48k"),
        decoded_bytes_24k: integer(total, "decoded_bytes_24k"),
        files: files.collect(),
        stereo_groups,
        sprite_maps,
        not_covered: paths(total, "not_covered"),
    }
}

fn stereo_class_named(name: &str) -> StereoClass {
    [
        StereoClass::SafeToMakeMono,
        StereoClass::ChangesTheSound,
        StereoClass::Cancels,
    ]
    .into_iter()
    .find(|class| class.name() == name)
    .unwrap_or_else(|| panic!("{name} is not a stereo class"))
}

fn listed_sprite_map(row: &Table) -> SpriteMapSummary {
    let optional = |key| row.contains_key(key).then(|| integer(row, key));
    SpriteMapSummary {
        source: path(text(row, "source")),
        status: status_named(text(row, "status")),
        audio: row.get("representative").and_then(Value::as_str).map(path),
        silence_bp: optional("silence_bp").map_or(0, |bp| bp.try_into().unwrap()),
        silence_bytes_48k: optional("silence_bytes_48k").unwrap_or(0),
    }
}

fn duplicate_group(row: &Table) -> DuplicateGroup {
    DuplicateGroup {
        sha256: text(row, "sha256").to_owned(),
        bytes: integer(row, "size"),
        paths: paths(row, "paths"),
        keep: row.get("keep").and_then(Value::as_str).map(path),
        wasted_bytes: integer(row, "wasted_bytes"),
        decoded_bytes_saved: integer(row, "decoded_bytes_saved"),
    }
}

fn listed_row(row: &Table) -> RuleRow {
    let value = ["value_bytes", "value_bp", "value_groups"]
        .into_iter()
        .find(|key| row.contains_key(*key))
        .map(|key| match key {
            "value_bytes" => RuleValue::Bytes(integer(row, key)),
            "value_bp" => RuleValue::BasisPoints(integer(row, key)),
            _ => RuleValue::Count(integer(row, key)),
        });
    RuleRow {
        outcome: verdict_named(text(row, "outcome")),
        value,
        saved_bytes: integer(row, "saved_bytes"),
        saved_decoded_bytes: row
            .contains_key("saved_decoded_bytes")
            .then(|| integer(row, "saved_decoded_bytes")),
        items: row.contains_key("items").then(|| paths(row, "items")),
        reason: row.get("reason").and_then(Value::as_str).map(str::to_owned),
    }
}

/// The report as a row, with only the keys the listed row has.
fn actual_row(report: &RuleReport, row: &Table) -> RuleRow {
    let listed = |key| row.contains_key(key);
    RuleRow {
        outcome: report.finding.verdict,
        value: report.value,
        saved_bytes: report.saved_bytes(),
        saved_decoded_bytes: listed("saved_decoded_bytes").then(|| report.saved_decoded_bytes()),
        items: listed("items").then(|| report.items.clone()),
        reason: listed("reason")
            .then(|| report.finding.reason.clone())
            .flatten(),
    }
}

/// Only estimated rows: the others carry no sizes.
fn estimated_rows<'a>(table: &'a Table, key: &str) -> impl Iterator<Item = &'a Table> {
    rows(table, key)
        .into_iter()
        .filter(|row| text(row, "status") == "estimated")
}

fn pair(row: &Table, key: &str) -> [u32; 2] {
    let values = row.get(key).and_then(Value::as_array);
    let Some([first, second]) = values.map(Vec::as_slice) else {
        panic!("{key} is a pair");
    };
    let side = |value: &Value| {
        let integer = value.as_integer().expect("a pair of integers");
        u32::try_from(integer).expect("the integer fits in u32")
    };
    [side(first), side(second)]
}

fn small_integer(row: &Table, key: &str) -> u32 {
    u32::try_from(integer(row, key)).unwrap_or_else(|_| panic!("{key} fits in u32"))
}

fn verdict_named(name: &str) -> Verdict {
    Value::String(name.to_owned())
        .try_into()
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn status_named(name: &str) -> MeasureStatus {
    Value::String(name.to_owned())
        .try_into()
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn budget(keys: &str) -> Budget {
    let text = format!("schema_version = {SCHEMA_VERSION}\n{keys}");
    Budget::parse(&text, ConfigFormat::Toml).unwrap()
}

fn report(summaries: &Summaries, budget: Option<&Budget>, rule: RuleId) -> RuleReport {
    let reports = rules::reports(summaries, budget);
    report_for(&reports, rule).clone()
}

fn report_for(reports: &[RuleReport], rule: RuleId) -> &RuleReport {
    reports
        .iter()
        .find(|report| report.finding.rule == rule)
        .unwrap_or_else(|| panic!("no report for {rule}"))
}

fn verdict(summaries: &Summaries, budget: Option<&Budget>, rule: RuleId) -> Verdict {
    report(summaries, budget, rule).finding.verdict
}

fn fix_actions(report: &RuleReport) -> Vec<FixAction> {
    report.finding.fixes.iter().map(|fix| fix.action).collect()
}

fn decoded_audio(bytes_48k: u64) -> Summaries {
    Summaries {
        audio: AudioSummary {
            decoded_bytes_48k: bytes_48k,
            decoded_bytes_24k: bytes_48k / 2,
            ..AudioSummary::default()
        },
        ..Summaries::default()
    }
}

fn render_sites(render_sites: Vec<RenderSite>) -> Summaries {
    Summaries {
        bundle: BundleSummary {
            render_sites,
            ..BundleSummary::default()
        },
        ..Summaries::default()
    }
}

fn report_of_render(render_sites: Vec<RenderSite>) -> RuleReport {
    report(
        &self::render_sites(render_sites),
        None,
        RuleId::RenderAntialiasOrResolution,
    )
}

/// A WebGL render site on the Reels frame. A resolution of None is one set at run time.
fn render_site(file: &str, antialias: Option<bool>, resolution_milli: Option<u32>) -> RenderSite {
    let frame = HostProfile::reels().viewport;
    let resolution = resolution_milli.map_or_else(Resolution::default, Resolution::exactly);
    RenderSite {
        file: path(file),
        offset: 0,
        antialias,
        draws_in_2d: false,
        resolution,
        canvas_css: frame,
        canvas: Some(canvas_floor(frame, resolution.floor_milli())),
    }
}

fn duplicates(duplicates: Vec<DuplicateGroup>) -> Summaries {
    Summaries {
        duplicates,
        ..Summaries::default()
    }
}

/// Two copies of one file, where the build names the first, with the decoded bytes the second
/// one wastes.
fn duplicate(paths: [&str; 2], bytes: u64, decoded_bytes: u64) -> DuplicateGroup {
    DuplicateGroup {
        sha256: "ab".repeat(32),
        bytes,
        keep: Some(path(paths[0])),
        paths: paths.map(path).to_vec(),
        wasted_bytes: bytes,
        decoded_bytes_saved: decoded_bytes,
    }
}

fn sprite_maps(sprite_maps: Vec<SpriteMapSummary>) -> Summaries {
    Summaries {
        audio: AudioSummary {
            sprite_maps,
            ..AudioSummary::default()
        },
        ..Summaries::default()
    }
}

fn sprite_map(source: &str, silence_bp: u32, silence_bytes_48k: u64) -> SpriteMapSummary {
    SpriteMapSummary {
        source: path(source),
        status: MeasureStatus::Estimated,
        audio: Some(path("sprites.mp3")),
        silence_bp,
        silence_bytes_48k,
    }
}
