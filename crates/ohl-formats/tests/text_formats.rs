//! Unit and `proptest` coverage for the game text/data format decoders:
//! `titles.txt`, `sentences.txt`, `skill.cfg`, `liblist.gam`, and the
//! shared `hud.txt`/`weapon_*.txt` grammar. See `docs/FORMAT_SOURCES.md`
//! ("Game text formats") for the public documentation these parsers were
//! implemented from.

use ohl_formats::hud_sprites::{self, Limits as HudLimits};
use ohl_formats::liblist::{self, Limits as LibListLimits};
use ohl_formats::sentences::{self, Limits as SentencesLimits};
use ohl_formats::skill_cfg::{self, Limits as SkillLimits};
use ohl_formats::titles::{self, Limits as TitlesLimits};
use proptest::prelude::*;

// ---------------------------------------------------------------------
// Unit tests over small, hand-written fixtures matching the documented
// grammar.
// ---------------------------------------------------------------------

#[test]
fn titles_parses_directives_and_blocks() {
    let data = b"$position 0 0\r\n$effect 2\r\n$color 255 128 0\r\nCHAPTER1_TITLE\r\n{\r\nChapter 1\r\nAnomalous Materials\r\n}\r\n$holdtime 5\r\nCHAPTER2_TITLE\r\n{\r\nChapter 2\r\n}\r\n";
    let limits = TitlesLimits::default();
    let file = titles::parse(data, &limits).expect("valid titles.txt parses");
    assert_eq!(file.messages().len(), 2);

    let first = file.find("CHAPTER1_TITLE").expect("first message present");
    assert_eq!(first.state.position, Some((0.0, 0.0)));
    assert_eq!(first.state.effect, Some(2));
    assert_eq!(first.state.color, Some((255, 128, 0)));
    assert_eq!(first.state.holdtime, None);
    assert_eq!(first.text_lossy(), "Chapter 1\nAnomalous Materials");

    let second = file.find("chapter2_title").expect("case-insensitive find");
    // Directive state carries forward across blocks.
    assert_eq!(second.state.color, Some((255, 128, 0)));
    assert_eq!(second.state.holdtime, Some(5.0));
    assert_eq!(second.text_lossy(), "Chapter 2");
}

#[test]
fn titles_drops_unterminated_block() {
    let data = b"NAME\n{\nsome text\n";
    let file = titles::parse(data, &TitlesLimits::default()).unwrap();
    assert!(file.messages().is_empty());
}

#[test]
fn sentences_parses_words_and_modifiers() {
    let data = b"// a leading comment\nHEV_HEALTH_1 hev/hev_health(p95) dying(v80,p110)\n";
    let file = sentences::parse(data, &SentencesLimits::default()).unwrap();
    assert_eq!(file.sentences().len(), 1);
    let sentence = file.find("hev_health_1").unwrap();
    assert_eq!(sentence.words.len(), 2);
    assert_eq!(sentence.words[0].token, "hev/hev_health");
    assert_eq!(sentence.words[0].modifiers.pitch, Some(95));
    assert_eq!(sentence.words[1].token, "dying");
    assert_eq!(sentence.words[1].modifiers.volume, Some(80));
    assert_eq!(sentence.words[1].modifiers.pitch, Some(110));
}

#[test]
fn skill_cfg_parses_cvar_lines() {
    let data = b"// difficulty cvars\nsk_headcrab_health1 \"1\"\nsk_headcrab_health2 \"2\"\nsk_headcrab_health3 \"3\"\n";
    let cfg = skill_cfg::parse(data, &SkillLimits::default()).unwrap();
    assert_eq!(cfg.entries().len(), 3);
    assert_eq!(cfg.get("sk_headcrab_health2"), Some("2"));
    assert_eq!(cfg.get("sk_headcrab_health_missing"), None);
}

#[test]
fn liblist_parses_key_value_pairs() {
    let data =
        b"game \"Half-Life\"\nstartmap \"c0a0\"\ntrainmap \"t0a0\"\ntype \"singleplayer_only\"\n";
    let list = liblist::parse(data, &LibListLimits::default()).unwrap();
    assert_eq!(list.startmap(), Some("c0a0"));
    assert_eq!(list.trainmap(), Some("t0a0"));
    assert_eq!(list.game(), Some("Half-Life"));
    assert_eq!(list.game_type(), Some("singleplayer_only"));
}

#[test]
fn liblist_tolerates_tab_separator() {
    let data = b"startmap\t\"c0a0\"\n";
    let list = liblist::parse(data, &LibListLimits::default()).unwrap();
    assert_eq!(list.startmap(), Some("c0a0"));
}

#[test]
fn hud_sprites_parses_count_header_and_rows() {
    let data = b"2\n// comment\nweapon_crowbar 320 sprites/weapon_crowbar.spr 0 0 24 24\nweapon_9mmhandgun 640 sprites/weapon_9mmhandgun.spr 24 0 24 24\n";
    let list = hud_sprites::parse(data, &HudLimits::default()).unwrap();
    assert_eq!(list.declared_count, Some(2));
    assert_eq!(list.rows().len(), 2);
    let row = &list.rows()[0];
    assert_eq!(row.name, "weapon_crowbar");
    assert_eq!(row.resolution, 320);
    assert_eq!(row.sprite_file, "sprites/weapon_crowbar.spr");
    assert_eq!((row.x, row.y, row.w, row.h), (0, 0, 24, 24));
}

#[test]
fn hud_sprites_tolerates_missing_count_header() {
    let data = b"weapon_crowbar 320 sprites/weapon_crowbar.spr 0 0 24 24\n";
    let list = hud_sprites::parse(data, &HudLimits::default()).unwrap();
    assert_eq!(list.declared_count, None);
    assert_eq!(list.rows().len(), 1);
}

// ---------------------------------------------------------------------
// `proptest`-driven fuzzing over arbitrary bytes: every parser and every
// accessor it exposes must never panic, no matter how malformed the input.
// ---------------------------------------------------------------------

fn exercise_titles(data: &[u8]) {
    let limits = TitlesLimits::default();
    let Ok(file) = titles::parse(data, &limits) else {
        return;
    };
    for message in file.messages() {
        let _ = message.text_lossy();
        let _ = file.find(message.name);
    }
}

fn exercise_sentences(data: &[u8]) {
    let limits = SentencesLimits::default();
    let Ok(file) = sentences::parse(data, &limits) else {
        return;
    };
    for sentence in file.sentences() {
        let _ = file.find(sentence.name);
        for word in &sentence.words {
            let _ = word.token;
        }
    }
}

fn exercise_skill_cfg(data: &[u8]) {
    let limits = SkillLimits::default();
    let Ok(cfg) = skill_cfg::parse(data, &limits) else {
        return;
    };
    for entry in cfg.entries() {
        let _ = cfg.get(entry.cvar);
    }
}

fn exercise_liblist(data: &[u8]) {
    let limits = LibListLimits::default();
    let Ok(list) = liblist::parse(data, &limits) else {
        return;
    };
    let _ = list.startmap();
    let _ = list.trainmap();
    let _ = list.game();
    let _ = list.game_type();
    let _ = list.mpentity();
    for (key, _) in list.entries() {
        let _ = list.get(key);
    }
}

fn exercise_hud_sprites(data: &[u8]) {
    let limits = HudLimits::default();
    let Ok(list) = hud_sprites::parse(data, &limits) else {
        return;
    };
    let _ = list.declared_count;
    for row in list.rows() {
        let _ = (
            row.name,
            row.sprite_file,
            row.resolution,
            row.x,
            row.y,
            row.w,
            row.h,
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn titles_parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise_titles(&data);
    }

    #[test]
    fn sentences_parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise_sentences(&data);
    }

    #[test]
    fn skill_cfg_parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise_skill_cfg(&data);
    }

    #[test]
    fn liblist_parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise_liblist(&data);
    }

    #[test]
    fn hud_sprites_parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise_hud_sprites(&data);
    }
}
