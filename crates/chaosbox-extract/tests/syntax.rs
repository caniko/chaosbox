//! Gold cases for the certified declaration subset, including forbidden matches.

use std::collections::BTreeSet;
use chaosbox_core::{coverage::SyntaxStatus, EntityKind, RelationType};
use chaosbox_extract::{build_candidates, extract_file};

#[test]
fn lookalikes_and_unresolved_mentions_never_become_certified_edges() {
    for (path, text, expected) in [
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
        assert_eq!(ext.facts.len(), defs.len() + 1);
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
