//! Certified source declarations. These are syntax observations, not compiler
//! symbols: configuration, macro expansion and target resolution are absent.

use std::{collections::BTreeSet, ops::Range, path::Path};

use chaosbox_core::{
    coverage::{FileCoverage, SyntaxStatus},
    Entity, EntityKind, RelationType,
};

use crate::{extractors::span_of, Extraction, StructuralFact};

// Exact dependency versions are pinned in Cargo.toml. Bump the contract version
// whenever the declaration subset or its interpretation changes.
const RUST: &str = "declarations-v1/tree-sitter-0.25.10/rust-0.24.2";
const TS: &str = "declarations-v1/tree-sitter-0.25.10/typescript-0.23.2";
const TSX: &str = "declarations-v1/tree-sitter-0.25.10/tsx-0.23.2";
const NIX: &str = "declarations-v1/rnix-0.14.0";

pub(crate) fn extract(repo: &str, snapshot: &str, path: &str, text: &str) -> Option<Extraction> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    let producer = match ext.as_str() {
        "rs" => RUST,
        "ts" | "mts" | "cts" => TS,
        "tsx" => TSX,
        "nix" => NIX,
        _ => return None,
    };
    let mut pass = Pass::new(repo, snapshot, path, text, producer);
    let parsed = if ext == "nix" {
        pass.nix()
    } else {
        let language = match ext.as_str() {
            "rs" => tree_sitter_rust::LANGUAGE.into(),
            "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
            _ => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        };
        pass.tree_sitter(&language, ext == "rs")
    };
    if parsed {
        pass.module();
        pass.lexical_proposals();
    }
    pass.out.coverage.push(FileCoverage {
        file: path.to_owned(),
        producer: producer.to_owned(),
        status: if parsed {
            SyntaxStatus::Parsed
        } else {
            SyntaxStatus::ParseError
        },
        facts: pass.out.facts.len(),
    });
    Some(pass.out)
}

struct Pass<'a> {
    repo: &'a str,
    snapshot: &'a str,
    path: &'a str,
    text: &'a str,
    producer: &'a str,
    file_id: String,
    mentions: BTreeSet<String>,
    out: Extraction,
}

impl<'a> Pass<'a> {
    fn new(
        repo: &'a str,
        snapshot: &'a str,
        path: &'a str,
        text: &'a str,
        producer: &'a str,
    ) -> Self {
        let file = Entity::new(
            EntityKind::File,
            repo,
            snapshot,
            path,
            path,
            path,
            span_of(text, path, 0, 0),
        );
        Self {
            repo,
            snapshot,
            path,
            text,
            producer,
            file_id: file.id.clone(),
            mentions: BTreeSet::new(),
            out: Extraction {
                compiler: None,
                entities: vec![file],
                explicit_refs: Vec::new(),
                facts: Vec::new(),
                coverage: Vec::new(),
            },
        }
    }

    fn module(&mut self) {
        let name = Path::new(self.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(self.path);
        let module = Entity::new(
            EntityKind::Module,
            self.repo,
            self.snapshot,
            self.path,
            name,
            &format!("{}::{name}", self.path),
            span_of(self.text, self.path, 0, 0),
        );
        self.fact(&module, RelationType::Contains);
        self.out.entities.push(module);
    }

    fn definition(&mut self, range: Range<usize>) {
        let name = &self.text[range.clone()];
        let entity = Entity::new(
            EntityKind::Definition,
            self.repo,
            self.snapshot,
            self.path,
            name,
            &format!("{}::{name}", self.path),
            span_of(self.text, self.path, range.start, range.end),
        );
        self.fact(&entity, RelationType::Defines);
        self.out.entities.push(entity);
    }

    fn fact(&mut self, entity: &Entity, rel_type: RelationType) {
        self.out.facts.push(StructuralFact {
            from: self.file_id.clone(),
            to: entity.id.clone(),
            rel_type,
            text: self.text[entity.span.byte_start as usize..entity.span.byte_end as usize]
                .to_owned(),
            span: entity.span.clone(),
            producer: self.producer.to_owned(),
        });
    }

    fn import_proposal(&mut self, range: Range<usize>, name: &str) {
        let entity = Entity::new(
            EntityKind::Import,
            self.repo,
            self.snapshot,
            self.path,
            name,
            &format!("{}::import::{name}", self.path),
            span_of(self.text, self.path, range.start, range.end),
        );
        self.out
            .explicit_refs
            .push((self.file_id.clone(), entity.id.clone(), "imports".into()));
        self.out.entities.push(entity);
    }

    fn lexical_proposals(&mut self) {
        for entity in &self.out.entities {
            if entity.kind == EntityKind::Definition && self.mentions.contains(&entity.name) {
                self.out.explicit_refs.push((
                    self.file_id.clone(),
                    entity.id.clone(),
                    "references".into(),
                ));
            }
        }
    }

    fn tree_sitter(&mut self, language: &tree_sitter::Language, rust: bool) -> bool {
        let mut parser = tree_sitter::Parser::new();
        // A pinned grammar ABI mismatch is a programming fault, not a source
        // parse error; never disguise it as partial coverage.
        parser.set_language(language).expect("pinned grammar ABI");
        let Some(tree) = parser.parse(self.text, None) else {
            return false;
        };
        if tree.root_node().has_error() {
            return false;
        }
        // Iterative traversal bounds the Rust stack even for deeply nested code.
        let mut cursor = tree.walk();
        loop {
            let node = cursor.node();
            let name = declaration_name(node, rust);
            if let Some(name) = name {
                self.definition(name.byte_range());
            }
            if node.kind() == "use_declaration" {
                if let Some(target) = node.child_by_field_name("argument") {
                    let text = &self.text[target.byte_range()];
                    self.import_proposal(node.byte_range(), text);
                }
            } else if node.kind() == "import_statement" {
                if let Some(source) = node.child_by_field_name("source") {
                    let text = &self.text[source.byte_range()];
                    self.import_proposal(source.byte_range(), &text[1..text.len() - 1]);
                }
            } else if node.kind() == "identifier" && !is_declaration_name(node, rust) {
                self.mentions
                    .insert(self.text[node.byte_range()].to_owned());
            }
            // Token trees are unexpanded macro input. Never parse declarations
            // from macro text, comments, strings, or JSX text as Rust/TS items.
            if !matches!(
                node.kind(),
                "token_tree"
                    | "macro_definition"
                    | "string"
                    | "string_literal"
                    | "raw_string_literal"
            ) && cursor.goto_first_child()
            {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    return true;
                }
            }
        }
    }

    fn nix(&mut self) -> bool {
        use rnix::SyntaxKind::{NODE_ATTRPATH, NODE_ATTRPATH_VALUE, NODE_IDENT, NODE_PATH_REL};
        let parsed = rnix::Root::parse(self.text);
        if !parsed.errors().is_empty() {
            return false;
        }
        for node in parsed.syntax().descendants() {
            let range = node.text_range();
            let range = usize::from(range.start())..usize::from(range.end());
            match node.kind() {
                NODE_ATTRPATH_VALUE => {
                    if let Some(path) = node.children().find(|n| n.kind() == NODE_ATTRPATH) {
                        // Only static identifier paths are certified. Quoted or
                        // dynamic attributes, inherit and lambda parameters wait
                        // for a scope-aware resolver.
                        if path.children().all(|n| n.kind() == NODE_IDENT) {
                            let range = path.text_range();
                            self.definition(usize::from(range.start())..usize::from(range.end()));
                        }
                    }
                }
                NODE_PATH_REL if node.children().next().is_none() => {
                    let text = &self.text[range.clone()];
                    if Path::new(text)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("nix"))
                    {
                        self.import_proposal(range, text);
                    }
                }
                NODE_IDENT if node.parent().is_none_or(|p| p.kind() != NODE_ATTRPATH) => {
                    self.mentions.insert(self.text[range].to_owned());
                }
                _ => {}
            }
        }
        true
    }
}

fn declaration_name(node: tree_sitter::Node<'_>, rust: bool) -> Option<tree_sitter::Node<'_>> {
    let supported = if rust {
        matches!(
            node.kind(),
            "function_item"
                | "function_signature_item"
                | "struct_item"
                | "enum_item"
                | "trait_item"
                | "type_item"
                | "const_item"
                | "static_item"
                | "mod_item"
                | "union_item"
        )
    } else {
        matches!(
            node.kind(),
            "function_declaration"
                | "generator_function_declaration"
                | "function_signature"
                | "class_declaration"
                | "abstract_class_declaration"
                | "interface_declaration"
                | "type_alias_declaration"
                | "enum_declaration"
                | "method_definition"
                | "method_signature"
                | "variable_declarator"
        )
    };
    if !supported {
        return None;
    }
    node.child_by_field_name("name").filter(|name| {
        matches!(
            name.kind(),
            "identifier" | "type_identifier" | "property_identifier"
        )
    })
}

fn is_declaration_name(node: tree_sitter::Node<'_>, rust: bool) -> bool {
    node.parent()
        .and_then(|parent| declaration_name(parent, rust))
        == Some(node)
}
