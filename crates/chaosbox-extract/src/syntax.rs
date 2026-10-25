//! Certified source declarations. These are syntax observations, not compiler
//! symbols: configuration, macro expansion and target resolution are absent.
//! Call and heritage *use sites* are collected here and resolved by name at
//! snapshot level (`resolve.rs`); that resolution is INFERRED, never certified.

use std::{collections::BTreeSet, ops::Range, path::Path, sync::LazyLock};

use chaosbox_core::{
    coverage::{FileCoverage, SyntaxStatus},
    Entity, EntityKind, RelationType, SourceSpan,
};
use regex::Regex;

use crate::{
    extractors::{item_text, span_of},
    Extraction, StructuralFact,
};

// Exact dependency versions are pinned in Cargo.toml. Bump the contract version
// whenever the declaration subset or its interpretation changes. v2 adds
// syntactic nesting (definition -> nested definition) and rationale comments.
const RUST: &str = "declarations-v2/tree-sitter-0.25.10/rust-0.24.2";
const TS: &str = "declarations-v2/tree-sitter-0.25.10/typescript-0.23.2";
const TSX: &str = "declarations-v2/tree-sitter-0.25.10/tsx-0.23.2";
const NIX: &str = "declarations-v1/rnix-0.14.0";

/// Marked comments that become rationale annotations: uppercase markers plus
/// the `ponytail:` simplification convention, first marker per comment. The
/// marker must open a comment line, so prose that merely mentions `NOTE:`
/// is not an annotation.
static RATIONALE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[\s/*!#]*\b(NOTE|WHY|TODO|FIXME|HACK|SAFETY|XXX|ponytail)(\([^)]*\))?:")
        .expect("static regex")
});

/// Source of a use site: the enclosing entity, or (Rust `impl Trait for
/// Type`) a type name resolved within the same file.
#[derive(Clone, Debug)]
pub(crate) enum SiteFrom {
    Entity(String),
    Name(String),
}

/// A call or heritage use site awaiting snapshot-level name resolution.
#[derive(Clone, Debug)]
pub(crate) struct Site {
    pub rel: RelationType,
    pub from: SiteFrom,
    /// Target name exactly as written at `span`.
    pub name: String,
    pub span: SourceSpan,
    /// Last segment of a path qualifier (`Type` in `Type::new`), if any.
    pub qualifier: Option<String>,
    /// `self.`/`this.`/`Self::`/`self::`: resolve within this file only.
    pub local: bool,
    pub rust: bool,
    pub producer: &'static str,
}

/// One file's use sites plus the declarations each site kind may target.
#[derive(Debug, Default)]
pub(crate) struct Uses {
    pub sites: Vec<Site>,
    /// Definitions a call may name: functions, methods, constructors.
    pub callable: BTreeSet<String>,
    /// Definitions a heritage clause or impl may name: types and traits.
    pub types: BTreeSet<String>,
}

pub(crate) fn extract(
    repo: &str,
    snapshot: &str,
    path: &str,
    text: &str,
) -> Option<(Extraction, Uses)> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    let producer = match ext.as_str() {
        "rs" => RUST,
        "ts" | "mts" | "cts" => TS,
        "tsx" => TSX,
        "nix" => NIX,
        _ => return None,
    };
    let mut pass = Pass::new(repo, snapshot, path, text, producer);
    pass.rust = ext == "rs";
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
    } else {
        // A rejected file certifies nothing, so it resolves nothing either.
        pass.uses = Uses::default();
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
    Some((pass.out, pass.uses))
}

struct Pass<'a> {
    repo: &'a str,
    snapshot: &'a str,
    path: &'a str,
    text: &'a str,
    producer: &'static str,
    file_id: String,
    mentions: BTreeSet<String>,
    /// Enclosing declarations as `(end byte, entity id)`, innermost last.
    scopes: Vec<(usize, String)>,
    uses: Uses,
    rust: bool,
    out: Extraction,
}

impl<'a> Pass<'a> {
    fn new(
        repo: &'a str,
        snapshot: &'a str,
        path: &'a str,
        text: &'a str,
        producer: &'static str,
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
            scopes: Vec::new(),
            uses: Uses::default(),
            rust: false,
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

    fn def_entity(&self, range: Range<usize>) -> Entity {
        let name = &self.text[range.clone()];
        Entity::new(
            EntityKind::Definition,
            self.repo,
            self.snapshot,
            self.path,
            name,
            &format!("{}::{name}", self.path),
            span_of(self.text, self.path, range.start, range.end),
        )
    }

    /// `item` is the whole declaration, captured as bounded entity text.
    /// `scope_end` (the declaration node's end) enables syntactic nesting;
    /// Nix passes `None`: its attribute paths are not scoped here.
    fn definition(
        &mut self,
        range: Range<usize>,
        item: Range<usize>,
        scope_end: Option<usize>,
    ) -> String {
        let mut entity = self.def_entity(range);
        entity.text = item_text(self.text, item.start, item.end);
        let id = entity.id.clone();
        self.fact(&entity, RelationType::Defines);
        if let Some(end) = scope_end {
            if let Some((_, parent)) = self.scopes.last() {
                let parent = parent.clone();
                self.fact_from(parent, &entity, RelationType::Contains);
            }
            self.scopes.push((end, entity.id.clone()));
        }
        self.out.entities.push(entity);
        id
    }

    /// Record which declarations calls and heritage clauses may resolve to,
    /// so `Err(..)` never resolves to an associated `type Err`.
    fn classify(&mut self, node_kind: &str, id: String) {
        if matches!(
            node_kind,
            "function_item"
                | "function_signature_item"
                | "struct_item"
                | "function_declaration"
                | "generator_function_declaration"
                | "function_signature"
                | "method_definition"
                | "method_signature"
                | "class_declaration"
                | "abstract_class_declaration"
                | "variable_declarator"
        ) {
            self.uses.callable.insert(id.clone());
        }
        if matches!(
            node_kind,
            "struct_item"
                | "enum_item"
                | "trait_item"
                | "type_item"
                | "union_item"
                | "class_declaration"
                | "abstract_class_declaration"
                | "interface_declaration"
                | "type_alias_declaration"
        ) {
            self.uses.types.insert(id);
        }
    }

    fn enclosing(&self) -> String {
        self.scopes
            .last()
            .map_or_else(|| self.file_id.clone(), |(_, id)| id.clone())
    }

    /// A marked comment annotates the next declaration, else its enclosing
    /// declaration, else the file. It is never a declaration itself.
    fn rationale(&mut self, node: tree_sitter::Node<'_>) {
        let Some(found) = RATIONALE.captures(&self.text[node.byte_range()]) else {
            return;
        };
        let start = node.start_byte() + found.get(1).map_or(0, |m| m.start());
        let line_end = self.text[start..node.end_byte()]
            .find('\n')
            .map_or(node.end_byte(), |i| start + i);
        let end = start
            + self.text[start..line_end]
                .trim_end()
                .trim_end_matches("*/")
                .trim_end()
                .len();
        let target = next_declaration(node, self.rust).map_or_else(
            || self.enclosing(),
            |name| self.def_entity(name.byte_range()).id,
        );
        let entity = Entity::new(
            EntityKind::Rationale,
            self.repo,
            self.snapshot,
            self.path,
            &self.text[start..end],
            &format!("{}::{}", self.path, &found[1]),
            span_of(self.text, self.path, start, end),
        );
        self.fact_from(target, &entity, RelationType::Contains);
        self.out.entities.push(entity);
    }

    fn site(
        &mut self,
        rel: RelationType,
        from: SiteFrom,
        name: tree_sitter::Node<'_>,
        qualifier: Option<tree_sitter::Node<'_>>,
        local: bool,
    ) {
        let text = &self.text[name.byte_range()];
        if text.is_empty() || matches!(text, "self" | "Self" | "super" | "crate") {
            return;
        }
        self.uses.sites.push(Site {
            rel,
            from,
            name: text.to_owned(),
            span: span_of(self.text, self.path, name.start_byte(), name.end_byte()),
            qualifier: qualifier.map(|q| self.text[q.byte_range()].to_owned()),
            local,
            rust: self.rust,
            producer: self.producer,
        });
    }

    /// Call sites with a syntactically nameable callee. Method calls on other
    /// receivers (`x.len()`) are never collected: receiver typing is SCIP's job.
    fn call(&mut self, callee: tree_sitter::Node<'_>) {
        let from = SiteFrom::Entity(self.enclosing());
        match callee.kind() {
            "identifier" => self.site(RelationType::Calls, from, callee, None, false),
            "generic_function" => {
                if let Some(inner) = callee.child_by_field_name("function") {
                    self.call(inner);
                }
            }
            "scoped_identifier" => {
                let Some(name) = callee.child_by_field_name("name") else {
                    return;
                };
                let qualifier = callee.child_by_field_name("path").map(path_name);
                let local = qualifier
                    .is_some_and(|q| matches!(&self.text[q.byte_range()], "self" | "Self"));
                self.site(RelationType::Calls, from, name, qualifier, local);
            }
            "field_expression" | "member_expression" => {
                let (object, field) = if callee.kind() == "field_expression" {
                    ("value", "field")
                } else {
                    ("object", "property")
                };
                let receiver = callee.child_by_field_name(object).map(|n| n.kind());
                if matches!(receiver, Some("self" | "this")) {
                    if let Some(name) = callee.child_by_field_name(field) {
                        self.site(RelationType::Calls, from, name, None, true);
                    }
                }
            }
            _ => {}
        }
    }

    fn heritage(&mut self, rel: RelationType, ty: tree_sitter::Node<'_>) {
        if let Some((name, qualifier)) = type_name(ty) {
            let from = SiteFrom::Entity(self.enclosing());
            self.site(rel, from, name, qualifier, false);
        }
    }

    /// Use sites carried by `node` itself (calls, heritage, impl headers).
    fn uses(&mut self, node: tree_sitter::Node<'_>) {
        let mut walk = node.walk();
        match node.kind() {
            "line_comment" | "block_comment" | "comment" => self.rationale(node),
            "call_expression" => {
                if let Some(callee) = node.child_by_field_name("function") {
                    self.call(callee);
                }
            }
            "new_expression" => {
                if let Some(ctor) = node
                    .child_by_field_name("constructor")
                    .filter(|n| n.kind() == "identifier")
                {
                    let from = SiteFrom::Entity(self.enclosing());
                    self.site(RelationType::Calls, from, ctor, None, false);
                }
            }
            "extends_clause" => {
                for value in node.children_by_field_name("value", &mut walk) {
                    self.heritage(RelationType::Inherits, value);
                }
            }
            "extends_type_clause" => {
                for ty in node.children_by_field_name("type", &mut walk) {
                    self.heritage(RelationType::Inherits, ty);
                }
            }
            "implements_clause" => {
                for ty in node.named_children(&mut walk) {
                    self.heritage(RelationType::Implements, ty);
                }
            }
            "impl_item" => {
                let names = node
                    .child_by_field_name("trait")
                    .and_then(type_name)
                    .zip(node.child_by_field_name("type").and_then(type_name));
                if let Some(((trait_name, qualifier), (type_node, _))) = names {
                    let from = SiteFrom::Name(self.text[type_node.byte_range()].to_owned());
                    self.site(RelationType::Implements, from, trait_name, qualifier, false);
                }
            }
            _ => {}
        }
    }

    fn fact(&mut self, entity: &Entity, rel_type: RelationType) {
        self.fact_from(self.file_id.clone(), entity, rel_type);
    }

    fn fact_from(&mut self, from: String, entity: &Entity, rel_type: RelationType) {
        self.out.facts.push(StructuralFact {
            from,
            to: entity.id.clone(),
            rel_type,
            text: self.text[entity.span.byte_start as usize..entity.span.byte_end as usize]
                .to_owned(),
            span: entity.span.clone(),
            producer: self.producer.to_owned(),
            resolved: false,
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
            while self
                .scopes
                .last()
                .is_some_and(|(end, _)| *end <= node.start_byte())
            {
                self.scopes.pop();
            }
            let name = declaration_name(node, rust);
            if let Some(name) = name {
                let id =
                    self.definition(name.byte_range(), node.byte_range(), Some(node.end_byte()));
                self.classify(node.kind(), id);
            }
            self.uses(node);
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
                            let name = path.text_range();
                            self.definition(
                                usize::from(name.start())..usize::from(name.end()),
                                range.clone(),
                                None,
                            );
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

/// The declaration a leading comment documents: the next named sibling,
/// skipping comments, attributes and decorators, looking into `export` and
/// `const`/`let` wrappers.
fn next_declaration(node: tree_sitter::Node<'_>, rust: bool) -> Option<tree_sitter::Node<'_>> {
    let mut next = node.next_named_sibling();
    while let Some(sibling) = next {
        if matches!(
            sibling.kind(),
            "line_comment" | "block_comment" | "comment" | "attribute_item" | "decorator"
        ) {
            next = sibling.next_named_sibling();
            continue;
        }
        let mut inner = sibling;
        if inner.kind() == "export_statement" {
            inner = inner.child_by_field_name("declaration")?;
        }
        if inner.kind() == "lexical_declaration" {
            let mut walk = inner.walk();
            inner = inner
                .named_children(&mut walk)
                .find(|n| n.kind() == "variable_declarator")?;
        }
        return declaration_name(inner, rust);
    }
    None
}

/// Last segment of a Rust path qualifier (`a::B<T>` -> `B`).
fn path_name(mut node: tree_sitter::Node<'_>) -> tree_sitter::Node<'_> {
    loop {
        let inner = match node.kind() {
            "scoped_identifier" | "scoped_type_identifier" => node.child_by_field_name("name"),
            "generic_type" => node.child_by_field_name("type"),
            _ => None,
        };
        match inner {
            Some(inner) => node = inner,
            None => return node,
        }
    }
}

/// Name node (and path qualifier) of a heritage or impl type expression.
fn type_name(
    node: tree_sitter::Node<'_>,
) -> Option<(tree_sitter::Node<'_>, Option<tree_sitter::Node<'_>>)> {
    match node.kind() {
        "identifier" | "type_identifier" => Some((node, None)),
        // TS names the generic's base `name`, Rust names it `type`.
        "generic_type" => node
            .child_by_field_name("name")
            .or_else(|| node.child_by_field_name("type"))
            .and_then(type_name),
        "scoped_type_identifier" => node
            .child_by_field_name("name")
            .map(|name| (name, node.child_by_field_name("path").map(path_name))),
        "nested_type_identifier" => node.child_by_field_name("name").map(|name| (name, None)),
        "member_expression" => node
            .child_by_field_name("property")
            .map(|name| (name, None)),
        _ => None,
    }
}

fn is_declaration_name(node: tree_sitter::Node<'_>, rust: bool) -> bool {
    node.parent()
        .and_then(|parent| declaration_name(parent, rust))
        == Some(node)
}
