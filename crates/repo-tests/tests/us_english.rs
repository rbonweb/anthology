//! First-party text is US English: no UK spelling in a tracked text file (owner request,
//! 2026-11-28).
//!
//! Every tracked UTF-8 file is read except the excluded paths: upstream and generated text
//! (`vendor/`, the superpowers plugin, the generated knowledge folders, the whole of `atl/` except
//! the files this fork added), the GuardDog rule files vendored into the check-repo scanner, and
//! the `.gz` goldens, which only `golden bless` rewrites. Each line is cut into identifiers, and each identifier into words
//! (`SanitiseProse` is `Sanitise` and `Prose`); a word that is a UK form from the list below fails
//! the test with its file, line, the word and its US form.
//!
//! Two things stay UK on purpose and are exempt:
//! - the inherited Python test names that `port/` pins (`test_allowed_g1_neighbours`): the port
//!   ledger ties each Rust test to its Python name, so the name must not change;
//! - the contract strings in [`ALLOWED`], one row each with the reason: a name another tool reads
//!   (a GitHub Actions function, a Claude Code JSON value, a command-line flag, a file name the
//!   slop gate exempts, a golden case name).
//!
//! This file names the UK spellings itself, so it is an excluded path.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use repo_tests::repo;

/// This file, which has to spell the UK forms to look for them.
const THIS_FILE: &str = "crates/repo-tests/tests/us_english.rs";

/// Folders and files whose text is not ours to edit, or is written by a tool.
const EXCLUDED: [&str; 8] = [
    "vendor/",
    "plugins/anthology-superpowers/",
    "plugins/anthology/knowledge/",
    // The check-repo scanner's vendored GuardDog rules: upstream text (Apache-2.0, DataDog/guarddog),
    // kept unchanged so a later sync can diff against it (see `SOURCE` in the folder). The rest of
    // the scanner crate is ours and is checked.
    "crates/anthology-scan/rules/guarddog/",
    // The Python test names and rows that `port/ledger` maps to Rust tests.
    "port/python-test-ids.txt",
    "port/python-test-rows.tsv",
    "port/ledger/",
    // Rows copied from the Python guard tests; their case names are the inherited test names.
    "crates/anthology-guard/tests/data/",
];

/// `atl/` is the upstream tool, a fork; only these files are the fork's own.
const ATL_OWN: [&str; 5] = [
    "atl/src/hooks/anthology_gate.rs",
    "atl/src/hooks/read_only_proof.rs",
    "atl/src/hooks/user_allow.rs",
    "atl/tests/anthology_gate_test.rs",
    "atl/tests/anthology_shell_atl_forms_test.rs",
];

/// `(file, word in lower case, why it stays)`: a UK spelling that is a name another tool reads.
/// Every row must still be needed: the test fails on a row nothing uses.
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        ".github/workflows/ci.yml",
        "cancelled",
        "`cancelled()` is the GitHub Actions status function",
    ),
    (
        ".github/workflows/upstream-watch.yml",
        "cancelled",
        "`cancelled()` is the GitHub Actions status function",
    ),
    (
        "crates/repo-tests/tests/rust_test_jobs.rs",
        "cancelled",
        "pins the GitHub Actions `cancelled()` function and the job result `cancelled`",
    ),
    (
        "crates/anthology-dev/tests/smoke.rs",
        "cancelled",
        "a Claude Code `hook_response` event with the value `\"outcome\": \"cancelled\"`",
    ),
    (
        "crates/anthology-dev/src/smoke/helpers.rs",
        "cancelled",
        "quotes the Claude Code `outcome` value `cancelled`",
    ),
    (
        "crates/anthology-guard/tests/known_bypasses.rs",
        "colour",
        "`--colour=never` is the ugrep command-line flag",
    ),
    (
        "crates/anthology-slop/src/paths.rs",
        "licence",
        "the slop gate exempts a file named LICENCE as well as LICENSE",
    ),
    (
        "docs/slop-gate.md",
        "licence",
        "lists the exempt file name LICENCE",
    ),
    (
        "crates/anthology-slop/src/rules/text.rs",
        "revolutionise",
        "the tell the rule flags is spelled both ways, and its test needs the UK form",
    ),
    (
        "crates/repo-tests/tests/knowledge.rs",
        "licence",
        "\"Open Government Licence v3.0\" is the name of a third-party license",
    ),
    (
        "docs/superpowers/plans/2026-11-28-copy-and-marketing.md",
        "licence",
        "quotes \"Open Government Licence v3.0\", the name of a third-party license, and the line a test pins",
    ),
    (
        "crates/difftest/src/surfaces/dev_upstream_watch.rs",
        "sanitise",
        "`sanitise` is an op name inside the recorded golden cases",
    ),
];

/// The verbs with a `-ise` UK spelling, up to the `-is`; the suffix is added in `WORD_FORMS`.
const STEMS: &str = "organis|recognis|summaris|prioritis|normalis|initialis|serialis|customis|\
optimis|authoris|minimis|maximis|finalis|generalis|utilis|visualis|realis|specialis|sanitis|\
tokenis|memois|memoris|capitalis|categoris|characteris|synchronis|randomis|materialis|standardis|\
localis|centralis|stylis|legitimis|apologis|criticis|equalis|formalis|globalis|virtualis|digitis|\
itemis|mobilis|personalis|privatis|stabilis|symbolis|neutralis|internalis|externalis|parameteris|\
containeris|anonymis|italicis|revolutionis|emphasis|hypothesis|synthesis|sympathis|scrutinis|\
harmonis|modernis|mechanis|industrialis|jeopardis|monetis|rationalis|socialis|subsidis|vandalis";

/// Words that are the same in one spelling and have one US form, with each form listed.
const PAIRS: &[(&str, &str)] = &[
    ("centre", "center"),
    ("centres", "centers"),
    ("centred", "centered"),
    ("centring", "centering"),
    ("catalogue", "catalog"),
    ("catalogues", "catalogs"),
    ("catalogued", "cataloged"),
    ("cataloguing", "cataloging"),
    ("dialogue", "dialog"),
    ("dialogues", "dialogs"),
    ("artefact", "artifact"),
    ("artefacts", "artifacts"),
    ("judgement", "judgment"),
    ("judgements", "judgments"),
    ("whilst", "while"),
    ("amongst", "among"),
    ("amidst", "amid"),
    ("programme", "program"),
    ("programmes", "programs"),
    ("grey", "gray"),
    ("greys", "grays"),
    ("greyed", "grayed"),
    ("greyish", "grayish"),
    ("greying", "graying"),
    ("fulfil", "fulfill"),
    ("fulfils", "fulfills"),
    ("enrol", "enroll"),
    ("enrols", "enrolls"),
    ("defence", "defense"),
    ("defences", "defenses"),
    ("defenceless", "defenseless"),
    ("offence", "offense"),
    ("offences", "offenses"),
    ("practise", "practice"),
    ("practised", "practiced"),
    ("practises", "practices"),
    ("practising", "practicing"),
    ("learnt", "learned"),
    ("spelt", "spelled"),
    ("licence", "license"),
    ("licences", "licenses"),
    ("licenced", "licensed"),
    ("licencing", "licensing"),
    ("licencee", "licensee"),
    ("aluminium", "aluminum"),
    ("sceptic", "skeptic"),
    ("sceptics", "skeptics"),
    ("sceptical", "skeptical"),
    ("scepticism", "skepticism"),
    ("tyre", "tire"),
    ("tyres", "tires"),
    ("cheque", "check"),
    ("cheques", "checks"),
    ("plough", "plow"),
    ("ploughs", "plows"),
    ("mould", "mold"),
    ("moulds", "molds"),
    ("moulded", "molded"),
    ("moulding", "molding"),
    ("sulphur", "sulfur"),
    ("kerb", "curb"),
    ("draught", "draft"),
    ("draughts", "drafts"),
    ("anticlockwise", "counterclockwise"),
    ("analyse", "analyze"),
    ("analysed", "analyzed"),
    ("analysing", "analyzing"),
    ("paralyse", "paralyze"),
    ("paralysed", "paralyzed"),
    ("catalyse", "catalyze"),
    ("catalysed", "catalyzed"),
    ("manoeuvre", "maneuver"),
    ("manoeuvred", "maneuvered"),
    ("encyclopaedia", "encyclopedia"),
    ("paediatric", "pediatric"),
    ("anaemia", "anemia"),
];

/// UK spellings that end `-our`; the US form drops the `u`. Matched anywhere in a word.
const OUR_WORDS: &str = "behaviour|colour|favour|honour|flavour|labour|neighbour|rumour|harbour|\
humour|endeavour|armour|vapour|saviour|odour|rigour|vigour|savour|tumour";

/// Verbs that double the final `l` in the UK past tense and `-ing` form (US: one `l`).
const DOUBLE_L: &str = "cancel|label|model|travel|signal|level|fuel|channel|counsel|tunnel|\
marshal|total|equal|quarrel|dial|pencil|panel";

static OUR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^(.*?)({OUR_WORDS})(.*)$")).unwrap());
static IZE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(un|re|de|dis|mis|over|under|pre|non|co)?({STEMS})(e|es|ed|ing|er|ers|ation|ations|able|ability)$"
    ))
    .unwrap()
});
static ISATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+)isation(s?)$").unwrap());
static LL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^(un)?({DOUBLE_L})l(ed|ing|er|ers)$")).unwrap());
static IDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9_]+").unwrap());

/// The US form of a lower-case word, or `None` when it is not a UK form on the list.
fn us_form(word: &str) -> Option<String> {
    if let Some(c) = IZE.captures(word) {
        let stem = &c[2];
        let prefix = c.get(1).map_or("", |m| m.as_str());
        return Some(format!("{prefix}{}z{}", &stem[..stem.len() - 1], &c[3]));
    }
    if let Some(c) = ISATION.captures(word) {
        return Some(format!("{}ization{}", &c[1], &c[2]));
    }
    if let Some(c) = LL.captures(word) {
        let prefix = c.get(1).map_or("", |m| m.as_str());
        return Some(format!("{prefix}{}{}", &c[2], &c[3]));
    }
    if let Some(c) = OUR.captures(word) {
        let ours = c[2].replace("our", "or");
        return Some(format!("{}{ours}{}", &c[1], &c[3]));
    }
    PAIRS
        .iter()
        .find(|(uk, _)| *uk == word)
        .map(|(_, us)| (*us).to_string())
}

/// `us` in the letter case of `word`: `Colour` gives `Color`, `COLOUR` gives `COLOR`.
fn like_case(word: &str, us: &str) -> String {
    if word.chars().count() > 1 && word.chars().all(|c| c.is_ascii_uppercase()) {
        return us.to_ascii_uppercase();
    }
    if word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        let mut chars = us.chars();
        return chars
            .next()
            .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
            .unwrap_or_default();
    }
    us.to_string()
}

/// The words in an identifier: split at `_` and digits and at a lower-to-upper letter change.
fn words_of(ident: &str) -> Vec<&str> {
    let bytes = ident.as_bytes();
    let mut words = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if !b.is_ascii_alphabetic() {
            if let Some(s) = start.take() {
                words.push(&ident[s..i]);
            }
            continue;
        }
        match start {
            None => start = Some(i),
            Some(s) => {
                let split = b.is_ascii_uppercase()
                    && (bytes[i - 1].is_ascii_lowercase()
                        || (bytes[i - 1].is_ascii_uppercase()
                            && bytes.get(i + 1).is_some_and(u8::is_ascii_lowercase)));
                if split {
                    words.push(&ident[s..i]);
                    start = Some(i);
                }
            }
        }
    }
    if let Some(s) = start {
        words.push(&ident[s..]);
    }
    words
}

/// Every UK word in `line` as `(the word, its US form)`; an identifier in `pinned` is skipped.
fn uk_words(line: &str, pinned: &HashSet<String>) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for ident in IDENT.find_iter(line) {
        if pinned.contains(ident.as_str()) {
            continue;
        }
        for word in words_of(ident.as_str()) {
            if let Some(us) = us_form(&word.to_ascii_lowercase()) {
                found.push((word.to_string(), like_case(word, &us)));
            }
        }
    }
    found
}

/// The identifiers in the port records (the Python test ids and the ledger) that hold a UK word:
/// the inherited test names, which keep their spelling so the ledger can tie them to Python.
fn pinned_names() -> HashSet<String> {
    let mut texts = vec![repo::read("port/python-test-ids.txt")];
    for entry in std::fs::read_dir(repo::path("port/ledger")).expect("port/ledger") {
        let path = entry.expect("an entry").path();
        if path.extension().is_some_and(|e| e == "tsv") {
            texts.push(std::fs::read_to_string(&path).expect("a ledger file"));
        }
    }
    let none = HashSet::new();
    texts
        .iter()
        .flat_map(|t| IDENT.find_iter(t).map(|m| m.as_str().to_string()))
        .filter(|ident| ident.starts_with("test_") && !uk_words(ident, &none).is_empty())
        .collect()
}

fn excluded(path: &str) -> bool {
    if path == THIS_FILE {
        return true;
    }
    if path.starts_with("atl/") {
        return !ATL_OWN.contains(&path);
    }
    EXCLUDED.iter().any(|e| {
        if e.ends_with('/') {
            path.starts_with(e)
        } else {
            path == *e
        }
    }) || path.ends_with(".gz")
        || path.ends_with(".lock")
}

/// The tracked files; with no git here (a remote build copies the tracked files without `.git`),
/// every file under the root outside `.git`, `target` and `node_modules`.
fn files() -> Vec<String> {
    if let Some(tracked) = repo::tracked_files() {
        return tracked;
    }
    fn visit(dir: &std::path::Path, rel: &str, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("a folder") {
            let entry = entry.expect("an entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let kind = entry.file_type().expect("a file type");
            if kind.is_dir() {
                if !matches!(name.as_str(), ".git" | "target" | "node_modules") {
                    visit(&entry.path(), &path, found);
                }
            } else if kind.is_file() {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    visit(&repo::root(), "", &mut found);
    found.sort();
    found
}

/// The text of a tracked file, or `None` for a file that is not UTF-8 text.
fn text_of(path: &str) -> Option<String> {
    let bytes = std::fs::read(repo::path(path)).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[test]
fn the_rules_say_what_they_flag() {
    let none = HashSet::new();
    for (uk, us) in [
        ("colour", "color"),
        ("Behaviour", "Behavior"),
        ("COLOURS", "COLORS"),
        ("neighbouring", "neighboring"),
        ("organisations", "organizations"),
        ("normalised", "normalized"),
        ("unrecognised", "unrecognized"),
        ("sanitisation", "sanitization"),
        ("emphasise", "emphasize"),
        ("analysing", "analyzing"),
        ("licence", "license"),
        ("cancelled", "canceled"),
        ("cancelling", "canceling"),
        ("labelled", "labeled"),
        ("unmodelled", "unmodeled"),
        ("centre", "center"),
        ("catalogue", "catalog"),
        ("dialogue", "dialog"),
        ("artefact", "artifact"),
        ("judgement", "judgment"),
        ("whilst", "while"),
        ("grey", "gray"),
        ("fulfil", "fulfill"),
        ("defence", "defense"),
        ("learnt", "learned"),
    ] {
        assert_eq!(
            uk_words(uk, &none),
            vec![(uk.to_string(), us.to_string())],
            "{uk}"
        );
    }
    for word in [
        "color",
        "behavior",
        "organize",
        "license",
        "canceled",
        "emphasis",
        "analysis",
        "analyses",
        "raise",
        "otherwise",
        "promise",
        "exercise",
        "precise",
        "noise",
        "disable",
        "cancellation",
        "specialist",
        "labelledby",
        "dialog",
        "center",
        "revise",
        "advise",
        "surprise",
        "premise",
        "expertise",
        "wise",
        "realistic",
        "organism",
    ] {
        assert!(uk_words(word, &none).is_empty(), "{word}");
    }
}

#[test]
fn identifiers_are_cut_into_words() {
    let none = HashSet::new();
    let words =
        |line: &str| -> Vec<String> { uk_words(line, &none).into_iter().map(|(w, _)| w).collect() };
    assert_eq!(words("fn sanitise_path(text: &str)"), ["sanitise"]);
    assert_eq!(words("SanitiseProse::new()"), ["Sanitise"]);
    assert_eq!(
        words("let guardBehaviour = HTMLColour;"),
        ["Behaviour", "Colour"]
    );
    assert_eq!(words("ugrep --colour=never"), ["colour"]);
    assert!(words("let analyses = recognizedNames;").is_empty());
}

#[test]
fn an_inherited_test_name_keeps_its_spelling() {
    let mut pinned = HashSet::new();
    pinned.insert("test_allowed_g1_neighbours".to_string());
    let line = "fn test_allowed_g1_neighbours() { neighbours(); }";
    assert_eq!(
        uk_words(line, &pinned),
        vec![("neighbours".to_string(), "neighbors".to_string())]
    );
    // The ledger and the Python ids do pin such names, and the real pins hold only such names.
    let pins = pinned_names();
    assert!(pins.contains("test_allowed_g1_neighbours"), "{pins:?}");
}

#[test]
fn the_exclusions_say_what_they_skip() {
    for path in [
        "vendor/sources.json",
        "plugins/anthology-superpowers/skills/x/SKILL.md",
        "plugins/anthology/knowledge/blog/SKILL.md",
        "crates/anthology-scan/rules/guarddog/threat-process-cryptomining.yar",
        "crates/anthology-scan/rules/guarddog/NOTICE",
        "atl/src/core/stream.rs",
        "crates/anthology-guard/tests/data/test_guard_rules.jsonl",
        "crates/anthology-core/tests/golden/x.jsonl.gz",
        "port/ledger/test_guard_rules.tsv",
        THIS_FILE,
    ] {
        assert!(excluded(path), "{path}");
    }
    for path in [
        "atl/src/hooks/anthology_gate.rs",
        "plugins/anthology/agents/architect.md",
        "plugins/anthology/hooks/mods/index.ts",
        "crates/anthology-cli/src/main.rs",
        "crates/anthology-scan/src/lib.rs",
        "crates/anthology-scan/NOTICE",
        "DECISIONS.md",
        ".claude/memory/MEMORY.md",
        "port/difftest/divergences/guard.hook.toml",
    ] {
        assert!(!excluded(path), "{path}");
    }
}

#[test]
fn first_party_text_has_no_uk_spelling() {
    let files = files();
    assert!(
        files.iter().any(|f| f == "Cargo.toml"),
        "not the repository root"
    );
    let pinned = pinned_names();
    let mut used = [false; ALLOWED.len()];
    let mut found: Vec<String> = Vec::new();
    for path in files.iter().filter(|p| !excluded(p)) {
        let Some(text) = text_of(path) else { continue };
        for (n, line) in text.lines().enumerate() {
            for (word, us) in uk_words(line, &pinned) {
                let lower = word.to_ascii_lowercase();
                let allowed = ALLOWED
                    .iter()
                    .position(|(file, w, _)| file == path && *w == lower);
                if let Some(i) = allowed {
                    used[i] = true;
                    continue;
                }
                found.push(format!(
                    "{path}:{}: `{word}` is UK spelling, write `{us}`",
                    n + 1
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "{} UK spelling(s) in first-party text; first 100:\n{}",
        found.len(),
        found
            .iter()
            .take(100)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    let stale: Vec<String> = ALLOWED
        .iter()
        .zip(used)
        .filter(|(_, used)| !used)
        .map(|((file, word, _), _)| format!("{file} {word}"))
        .collect();
    assert!(stale.is_empty(), "allow-list rows nothing uses: {stale:?}");
}

#[test]
fn every_allowed_row_has_a_reason_and_names_a_tracked_file() {
    let files = files();
    for &(file, word, why) in ALLOWED {
        assert!(files.iter().any(|f| f == file), "{file} is not tracked");
        assert!(
            us_form(word).is_some(),
            "{word} is not a UK word on the list"
        );
        assert!(why.len() > 20, "{file} {word}: say why it stays");
    }
}
