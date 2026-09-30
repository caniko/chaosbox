use std::collections::BTreeMap;
use chaosbox_core::{EntityKind, SourceSpan};
use chaosbox_extract::Snapshot;
use chaosbox_store::{MemoryStore, SourceCitation};
use super::{version::git_version, Endpoint, EndpointSpec, Member, Result, Spec, Workspace, CONTRACT};

/// Build an immutable selected-file investigation with source-grounded bridges.
pub async fn capture(spec: Spec) -> Result<Workspace> {
    super::validate_scope(&spec.scope, &spec.scope)?;
    super::validate_bounds(
        spec.members.len(),
        spec.endpoints.len(),
        spec.bridges.len(),
        spec.constraints.len(),
    )?;
    let mut members = BTreeMap::new();
    for (repo, selected) in spec.members {
        let root = selected.root.canonicalize()?;
        let snapshot = Snapshot::capture_files(&repo, &root, &selected.files)?;
        let extraction = chaosbox_extract::extract_snapshot(&snapshot);
        let mut pipe = crate::Pipeline::<MemoryStore>::new();
        let build = pipe
            .build_and_publish(
                &repo,
                &snapshot,
                &extraction,
                &[],
                &crate::Materialization::default(),
                None,
            )
            .await?;
        let (revision, dirty) = git_version(&root, &snapshot);
        members.insert(
            repo,
            Member {
                root,
                revision,
                dirty,
                snapshot,
                build,
            },
        );
    }
    let endpoints = spec
        .endpoints
        .into_iter()
        .map(|(id, source)| {
            let member = members
                .get(&source.member)
                .ok_or("endpoint member missing")?;
            endpoint(member, source).map(|endpoint| (id, endpoint))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut workspace = Workspace {
        id: String::new(),
        contract: CONTRACT.into(),
        scope: spec.scope,
        members,
        endpoints,
        bridges: spec.bridges,
        constraints: spec.constraints,
    };
    workspace.id = workspace.fingerprint()?;
    workspace.validate(&workspace.scope)?;
    if workspace
        .freshness()
        .values()
        .any(|status| status != "current")
    {
        return Err("selected inputs changed during capture".into());
    }
    Ok(workspace)
}

fn endpoint(member: &Member, source: EndpointSpec) -> Result<Endpoint> {
    if source.quote.is_empty() || source.quote.len() > 8192 {
        return Err("quote must contain 1..8192 bytes".into());
    }
    let text = member
        .snapshot
        .contents
        .get(&source.file)
        .ok_or("endpoint file outside selected inputs")?;
    let start = text
        .find(&source.quote)
        .ok_or("quote not found in source")?;
    // Comparing first/last also catches overlapping occurrences ("aaa" in "aaaa").
    if text.rfind(&source.quote) != Some(start) {
        return Err("quote is not unique in source".into());
    }
    let end = start + source.quote.len();
    let position = |byte| {
        let prefix = &text[..byte];
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((
            u32::try_from(prefix.bytes().filter(|b| *b == b'\n').count())? + 1,
            u32::try_from(prefix.rsplit('\n').next().unwrap_or("").chars().count())? + 1,
        ))
    };
    let (start_line, start_col) = position(start)?;
    let (end_line, end_col) = position(end)?;
    let entity = member
        .build
        .nodes
        .values()
        .find(|e| e.kind == EntityKind::File && e.file == source.file)
        .ok_or("endpoint lacks file entity")?;
    let version = member
        .snapshot
        .file_version(&source.file)
        .ok_or("endpoint lacks file version")?;
    Ok(Endpoint {
        member: source.member,
        build: member.build.id.clone(),
        entity: entity.id.clone(),
        citation: SourceCitation {
            snapshot: member.snapshot.id.clone(),
            file: source.file.clone(),
            sha256: version.sha256.clone(),
            span: Some(SourceSpan {
                file: source.file,
                start_line,
                start_col,
                end_line,
                end_col,
                byte_start: u32::try_from(start)?,
                byte_end: u32::try_from(end)?,
            }),
        },
        quote: source.quote,
    })
}
