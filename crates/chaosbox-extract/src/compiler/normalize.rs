use std::collections::{BTreeMap, BTreeSet};

use chaosbox_core::{
    compiler::{CompilerCoverage, CompilerFileCoverage, SymbolIdentity},
    sha256_hex, Entity, EntityKind, RelationType, SourceSpan,
};
use scip::types::{Document, Index, PositionEncoding};

use crate::{Snapshot, StructuralFact};
use super::{
    context::decode, invalid, positions::Positions, relative_path, CompilerError, Encoding, Receipt,
};

/// Normalized occurrences, independently certified facts, and coverage.
#[derive(Clone, Debug)]
pub struct CompilerExtraction {
    /// Compiler occurrences; syntax entities are not replaced or merged by name.
    pub entities: Vec<Entity>,
    /// File definitions, resolved references, explicit implementations.
    pub facts: Vec<StructuralFact>,
    /// Source/producer-bound coverage and capability limitations.
    pub coverage: CompilerCoverage,
}

impl CompilerExtraction {
    /// Add this one compiler analysis to syntax extraction. Candidate generation
    /// excludes compiler occurrences so they cannot create lexical proposals.
    pub fn attach(self, extraction: &mut crate::Extraction) -> Result<(), CompilerError> {
        if extraction.compiler.is_some() {
            return Err(invalid("one compiler context per build is supported"));
        }
        extraction.entities.extend(self.entities);
        extraction.facts.extend(self.facts);
        extraction.compiler = Some(self.coverage);
        Ok(())
    }
}

fn anchor(repo: &str, context: &str, file: &str, symbol: &str) -> Result<String, CompilerError> {
    scip::symbol::parse_symbol(symbol)
        .map_err(|e| invalid(format!("invalid SCIP symbol: {e:?}")))?;
    let parts = if scip::symbol::is_local_symbol(symbol) {
        vec![repo, context, file, symbol]
    } else {
        vec![repo, symbol]
    };
    Ok(format!("symbol:{}", sha256_hex(&parts)))
}

fn encoding(document: &Document, receipt: &Receipt) -> Result<Encoding, CompilerError> {
    match document.position_encoding.enum_value() {
        Ok(PositionEncoding::UTF8CodeUnitOffsetFromLineStart) => Ok(Encoding::Utf8),
        Ok(PositionEncoding::UTF16CodeUnitOffsetFromLineStart) => Ok(Encoding::Utf16),
        Ok(PositionEncoding::UTF32CodeUnitOffsetFromLineStart) => Ok(Encoding::Utf32),
        Ok(PositionEncoding::UnspecifiedPositionEncoding) => receipt
            .settings
            .unspecified_encoding
            .ok_or_else(|| invalid("unspecified position encoding requires an explicit override")),
        Err(_) => Err(invalid("unknown position encoding")),
    }
}

/// Decode genuine SCIP protobuf and normalize only receipt-pinned source bytes.
/// Call `receipt.verify(root)` at the filesystem ingestion boundary as well.
pub fn normalize(
    snapshot: &Snapshot,
    receipt: &Receipt,
    bytes: &[u8],
) -> Result<CompilerExtraction, CompilerError> {
    validate_snapshot(snapshot, receipt, bytes)?;
    let index = decode(bytes)?;
    let context = receipt.provenance();
    let mut output = CompilerExtraction {
        entities: Vec::new(),
        facts: Vec::new(),
        coverage: CompilerCoverage {
            context,
            files: Vec::new(),
            definitions: 0,
            references: 0,
            unresolved_references: 0,
            ambiguous_references: 0,
            symbol_less: 0,
            implementations: 0,
            unresolved_implementations: 0,
            unsupported_relationships: 0,
            diagnostics: 0,
            calls: "unsupported".into(),
            typecheck: "unknown".into(),
            configuration_membership: "unknown".into(),
        },
    };
    collect_documents(snapshot, receipt, &index, &mut output)?;
    output.entities.sort_by(|a, b| {
        (&a.file, a.span.byte_start, &a.id).cmp(&(&b.file, b.span.byte_start, &b.id))
    });
    output.entities.dedup_by(|a, b| a.id == b.id);
    let mut definitions: BTreeMap<&str, Vec<&Entity>> = BTreeMap::new();
    for entity in &output.entities {
        if entity.kind == EntityKind::Definition {
            definitions
                .entry(&identity(entity).anchor)
                .or_default()
                .push(entity);
            let file = Entity::new(
                EntityKind::File,
                &snapshot.repo,
                &snapshot.id,
                &entity.file,
                &entity.file,
                &entity.file,
                SourceSpan::point(&entity.file, 1, 1, 0),
            );
            output.facts.push(fact(
                &file.id,
                entity,
                RelationType::Defines,
                &output.coverage.context.producer,
                entity,
            ));
            output.coverage.definitions += 1;
        }
    }
    for entity in &output.entities {
        if entity.kind == EntityKind::Definition {
            continue;
        }
        match definitions
            .get(identity(entity).anchor.as_str())
            .map(Vec::as_slice)
        {
            Some([target]) => {
                output.facts.push(fact(
                    &entity.id,
                    target,
                    RelationType::References,
                    &output.coverage.context.producer,
                    entity,
                ));
                output.coverage.references += 1;
            }
            None | Some([]) => output.coverage.unresolved_references += 1,
            _ => output.coverage.ambiguous_references += 1,
        }
    }
    implementations(
        snapshot,
        receipt,
        &index,
        &definitions,
        &mut output.facts,
        &mut output.coverage,
    )?;
    output.facts.sort_by(|a, b| {
        (&a.from, &a.to, format!("{:?}", a.rel_type)).cmp(&(
            &b.from,
            &b.to,
            format!("{:?}", b.rel_type),
        ))
    });
    Ok(output)
}

fn validate_snapshot(
    snapshot: &Snapshot,
    receipt: &Receipt,
    bytes: &[u8],
) -> Result<(), CompilerError> {
    let verified = Receipt::seal(receipt.inputs.clone(), receipt.settings.clone(), bytes)?;
    if &verified != receipt {
        return Err(invalid("receipt fingerprint/index mismatch"));
    }
    if snapshot.id != receipt.inputs.snapshot
        || snapshot.repo != receipt.inputs.repo
        || snapshot.scope != receipt.inputs.scope
        || snapshot.files != receipt.inputs.sources
    {
        return Err(invalid("snapshot does not match compiler inputs"));
    }
    for file in &snapshot.files {
        if !snapshot.contents.get(&file.path).is_some_and(|text| {
            file.sha256 == sha256_hex(&[text]) && file.bytes == text.len() as u64
        }) {
            return Err(invalid("source content does not match captured hash"));
        }
    }
    Ok(())
}

fn identity(entity: &Entity) -> &SymbolIdentity {
    // Only compiler occurrences constructed below enter this module's maps.
    entity
        .compiler
        .as_ref()
        .expect("compiler occurrence identity")
}

fn collect_documents(
    snapshot: &Snapshot,
    receipt: &Receipt,
    index: &Index,
    output: &mut CompilerExtraction,
) -> Result<(), CompilerError> {
    let mut paths = BTreeSet::new();
    for document in &index.documents {
        let file = &document.relative_path;
        relative_path(file)?;
        if !paths.insert(file.clone()) {
            return Err(invalid("duplicate SCIP document"));
        }
        output.coverage.diagnostics += document
            .occurrences
            .iter()
            .map(|o| o.diagnostics.len())
            .sum::<usize>();
        let Some(text) = snapshot.contents.get(file) else {
            output.coverage.files.push(CompilerFileCoverage {
                file: file.clone(),
                status: "outside_source_scope".into(),
                encoding: None,
                occurrences: document.occurrences.len(),
            });
            continue;
        };
        if !document.text.is_empty() && document.text != *text {
            return Err(invalid("embedded document text differs from snapshot"));
        }
        let encoding = encoding(document, receipt)?;
        let positions = Positions::new(text)?;
        for occurrence in &document.occurrences {
            let span = positions.span(file, occurrence, encoding)?;
            if occurrence.symbol.is_empty() {
                output.coverage.symbol_less += 1;
                continue;
            }
            let symbol = &occurrence.symbol;
            let anchor = anchor(&snapshot.repo, &receipt.context, file, symbol)?;
            let name = &text[span.byte_start as usize..span.byte_end as usize];
            let kind = if occurrence.symbol_roles & 1 != 0 {
                EntityKind::Definition
            } else {
                EntityKind::Symbol
            };
            let mut entity =
                Entity::new(kind, &snapshot.repo, &snapshot.id, file, name, symbol, span);
            entity.id = format!(
                "ent:{}",
                sha256_hex(&[
                    &entity.id,
                    &receipt.context,
                    &entity.span.byte_end.to_string(),
                    &occurrence.symbol_roles.to_string()
                ])
            );
            entity.compiler = Some(SymbolIdentity {
                anchor,
                context: receipt.context.clone(),
                symbol: symbol.clone(),
                local: scip::symbol::is_local_symbol(symbol),
                roles: occurrence.symbol_roles,
            });
            output.entities.push(entity);
        }
        output.coverage.files.push(CompilerFileCoverage {
            file: file.clone(),
            status: "indexed".into(),
            encoding: Some(encoding.label().into()),
            occurrences: document.occurrences.len(),
        });
    }
    for file in &snapshot.files {
        if !paths.contains(&file.path) {
            output.coverage.files.push(CompilerFileCoverage {
                file: file.path.clone(),
                status: "omitted_by_indexer".into(),
                encoding: None,
                occurrences: 0,
            });
        }
    }
    output.coverage.files.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(())
}

fn fact(
    from: &str,
    to: &Entity,
    rel_type: RelationType,
    producer: &str,
    evidence: &Entity,
) -> StructuralFact {
    StructuralFact {
        from: from.into(),
        to: to.id.clone(),
        rel_type,
        span: evidence.span.clone(),
        text: evidence.name.clone(),
        producer: producer.into(),
    }
}

fn implementations(
    snapshot: &Snapshot,
    receipt: &Receipt,
    index: &Index,
    definitions: &BTreeMap<&str, Vec<&Entity>>,
    facts: &mut Vec<StructuralFact>,
    coverage: &mut CompilerCoverage,
) -> Result<(), CompilerError> {
    let mut seen = BTreeSet::new();
    for document in &index.documents {
        for info in &document.symbols {
            for relationship in &info.relationships {
                if !relationship.is_implementation {
                    coverage.unsupported_relationships += 1;
                    continue;
                }
                let from = anchor(
                    &snapshot.repo,
                    &receipt.context,
                    &document.relative_path,
                    &info.symbol,
                )?;
                let to = anchor(
                    &snapshot.repo,
                    &receipt.context,
                    &document.relative_path,
                    &relationship.symbol,
                )?;
                if !seen.insert((from.clone(), to.clone())) {
                    continue;
                }
                match (
                    definitions.get(from.as_str()).map(Vec::as_slice),
                    definitions.get(to.as_str()).map(Vec::as_slice),
                ) {
                    (Some([from]), Some([to]))
                        if snapshot.contents.contains_key(&document.relative_path) =>
                    {
                        facts.push(fact(
                            &from.id,
                            to,
                            RelationType::Implements,
                            &coverage.context.producer,
                            from,
                        ));
                        coverage.implementations += 1;
                    }
                    _ => coverage.unresolved_implementations += 1,
                }
            }
        }
    }
    coverage.unsupported_relationships += index
        .external_symbols
        .iter()
        .map(|s| s.relationships.len())
        .sum::<usize>();
    Ok(())
}
