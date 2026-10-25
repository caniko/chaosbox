//! Gold cases for the certified declaration subset, including forbidden matches.

use std::collections::BTreeSet;
use chaosbox_core::{coverage::SyntaxStatus, EntityKind, RelationType};
use chaosbox_extract::{build_candidates, extract_file};

#[test]
fn lookalikes_and_unresolved_mentions_never_become_certified_edges() {
    for (path, text, expected, nested) in [
        (
            "lib.rs",
            r##"
/* fn comment_fake() {} */
const TEXT: &str = r#"fn string_fake() {}"#;
pub fn real() { let _ = TEXT; }
mod nested { pub fn real() {} }
macro_rules! make { () => { fn macro_fake() {} }; }
"##,
            vec!["TEXT", "real", "nested", "real"],
            1,
        ),
        (
            "app.ts",
            r"
/* export function comment_fake() {} */
const text = `function string_fake() {}`;
export function real() { return text; }
export const arrow = () => text;
function scope() { const arrow = 1; }
",
            vec!["text", "real", "arrow", "scope", "arrow"],
            1,
        ),
        (
            "module.nix",
            r#"
{ # fake = 0;
 real = let real = 1; in real;
 text = ''
   string_fake = 2;
 '';
 imports = [ ./missing.nix ];
 ${"dynamic"} = 3;
}
"#,
            vec!["real", "real", "text", "imports"],
            0,
        ),
    ] {
        let ext = extract_file("r", "s", path, text);
        assert_eq!(ext.coverage[0].status, SyntaxStatus::Parsed, "{path}");
        let defs: Vec<_> = ext
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::Definition)
            .collect();
        assert_eq!(
            defs.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            expected,
            "{path}"
        );
        assert_eq!(
            defs.iter().map(|e| &e.id).collect::<BTreeSet<_>>().len(),
            defs.len(),
            "duplicate names retain occurrences"
        );
        // One `defines` per declaration, the file module, plus syntactic nesting.
        assert_eq!(ext.facts.len(), defs.len() + 1 + nested, "{path}");
        assert!(ext.facts.iter().all(|fact| matches!(
            fact.rel_type,
            RelationType::Defines | RelationType::Contains
        )));
        let candidates = build_candidates(&ext, 100);
        assert!(candidates
            .candidates
            .iter()
            .all(|c| !matches!(c.rel_type, RelationType::Defines | RelationType::Contains)));
    }
}

#[test]
fn invalid_files_have_explicit_zero_fact_coverage() {
    for (path, text) in [
        ("bad.rs", "fn broken("),
        ("bad.ts", "export function broken("),
        ("bad.nix", "{ broken = ; }"),
    ] {
        let ext = extract_file("r", "s", path, text);
        assert!(ext.facts.is_empty());
        assert_eq!(ext.coverage[0].status, SyntaxStatus::ParseError);
        assert_eq!(ext.entities.len(), 1, "only the captured file remains");
    }
}

#[test]
fn typescript_variants_and_unicode_ranges_are_exact() {
    for path in ["app.ts", "app.mts", "app.cts", "app.tsx"] {
        let text = "// 🦀\nexport function café() {}\n";
        let ext = extract_file("r", "s", path, text);
        let entity = ext.entities.iter().find(|e| e.name == "café").unwrap();
        assert_eq!(entity.span.start_line, 2);
        assert_eq!(entity.span.start_col, 17);
        assert_eq!(
            &text[entity.span.byte_start as usize..entity.span.byte_end as usize],
            "café"
        );
    }
    let tsx = extract_file(
        "r",
        "s",
        "view.tsx",
        "export const view = () => <span>function fake()</span>;",
    );
    assert_eq!(tsx.coverage[0].status, SyntaxStatus::Parsed);
    assert_eq!(
        tsx.entities
            .iter()
            .filter(|e| e.kind == EntityKind::Definition)
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["view"]
    );
}

#[test]
fn rationale_annotates_declarations_and_nesting_is_syntactic() {
    let rust = "mod m {\n    /// SAFETY: caller holds the lock\n    #[inline]\n    pub unsafe fn f() {}\n    // note: lowercase is ignored\n}\n\
                const S: &str = \"NOTE: not a comment\";\nfn g() {\n    // TODO: tidy */\n}\n";
    let ts = "class C {\n  // WHY: keeps order\n  m() {}\n  // mentions `NOTE:` in prose\n}\n";
    for (path, text, expected) in [
        (
            "lib.rs",
            rust,
            vec![
                ("m", "f", RelationType::Contains),
                ("f", "SAFETY: caller holds the lock", RelationType::Contains),
                ("g", "TODO: tidy", RelationType::Contains),
            ],
        ),
        (
            "app.ts",
            ts,
            vec![
                ("C", "m", RelationType::Contains),
                ("m", "WHY: keeps order", RelationType::Contains),
            ],
        ),
    ] {
        let ext = extract_file("r", "s", path, text);
        assert_eq!(ext.coverage[0].status, SyntaxStatus::Parsed, "{path}");
        let name = |id: &str| {
            ext.entities
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .name
                .clone()
        };
        let mut edges: Vec<_> = ext
            .facts
            .iter()
            .filter(|f| {
                ext.entities
                    .iter()
                    .any(|e| e.id == f.from && e.kind != EntityKind::File)
            })
            .map(|f| (name(&f.from), name(&f.to), format!("{:?}", f.rel_type)))
            .collect();
        let mut expected: Vec<_> = expected
            .into_iter()
            .map(|(a, b, r)| (a.to_owned(), b.to_owned(), format!("{r:?}")))
            .collect();
        edges.sort();
        expected.sort();
        assert_eq!(edges, expected, "{path}");
        assert!(ext.facts.iter().all(|f| !f.resolved));
    }
}
