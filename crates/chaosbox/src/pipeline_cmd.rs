//! The `run` command: snapshot, extract, decide, publish, and operator reporting.

use super::{
    Pipeline, Path, RunSpend, BTreeMap, Materialization, active_publishes_relations, LiveResponder,
    Ordering, FixtureResponder,
};

// Long CLI/dispatch functions; splitting them apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_pipeline_with<S: chaosbox_store::Store + Default>(
    mut pipe: Pipeline<S>,
    path: &Path,
    repo: &str,
    max_candidates: usize,
    effective_policy: &chaosbox_core::EffectivePolicy,
    live_jev: bool,
    fixture_decisions: bool,
    no_decisions: bool,
    max_requests: Option<u32>,
    max_input_tokens: Option<u64>,
    max_retries: Option<u32>,
    expected_predecessor: Option<String>,
    spend: &RunSpend,
    compiler: Option<&Path>,
) -> i32 {
    let (snap, mut ext, cat) =
        match Pipeline::<S>::snapshot_extract(repo, path, max_candidates, effective_policy) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("extract: {e}");
                return 1;
            }
        };
    if let Some(artifact) = compiler {
        if let Err(error) = chaosbox::compiler::attach(artifact, path, &snap, &mut ext) {
            eprintln!("compiler import: {error}");
            return 1;
        }
    }
    let cands = &cat.candidates;
    // Truncation must be observable: report what the cap selected and
    // omitted before any decision is made. Scope is part of the snapshot
    // identity, so it is reported here too: the same file set under a
    // different scope is a different snapshot (visible in `query status`
    // as a fingerprint change).
    eprintln!(
        "candidates: selected={} cap={} omitted={}",
        cands.len(),
        cat.cap,
        serde_json::to_string(&cat.omitted).unwrap(),
    );
    let resolved = ext.facts.iter().filter(|f| f.resolved).count();
    eprintln!(
        "structural: facts={} resolved={resolved} files={} parse_errors={}",
        ext.facts.len() - resolved,
        ext.coverage.len(),
        ext.coverage
            .iter()
            .filter(|f| f.status == chaosbox_core::coverage::SyntaxStatus::ParseError)
            .count()
    );
    if !snap.unreadable.is_empty() {
        eprintln!(
            "unreadable: skipped={} (not valid UTF-8) first={:?}",
            snap.unreadable.len(),
            &snap.unreadable[..snap.unreadable.len().min(5)]
        );
    }
    eprintln!(
        "scope: {}",
        if snap.scope.is_empty() {
            "(whole tree)".to_owned()
        } else {
            snap.scope.join(",")
        }
    );
    let endpoints: std::collections::BTreeSet<&str> = cands
        .iter()
        .flat_map(|candidate| [candidate.from_entity.as_str(), candidate.to_entity.as_str()])
        .collect();
    // Decision lookup needs only the bounded catalog's endpoints, not a second
    // owned copy of every entity in the uncapped structural graph.
    let entities: BTreeMap<_, _> = ext
        .entities
        .iter()
        .filter(|entity| endpoints.contains(entity.id.as_str()))
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let mat = Materialization::default();
    // Register file content identities before any evidence references them.
    if let Err(e) = pipe
        .store
        .ensure_snapshot_files(&snap.id, repo, &snap.snapshot_files())
        .await
    {
        eprintln!("snapshot files: {e}");
        return 1;
    }
    // Mint deterministic run/set identity and register the candidate catalog
    // before any decision references it.
    let digest = chaosbox_core::catalog_digest(cands);
    let run_id = chaosbox_core::deterministic_id("run", &[repo, &snap.id]);
    let set_id = chaosbox_core::deterministic_id("set", &[&run_id, &digest, &mat.rubric_version]);
    if let Err(e) = pipe
        .store
        .ensure_run(
            &run_id,
            repo,
            &snap.id,
            &set_id,
            &digest,
            &mat.rubric_version,
        )
        .await
    {
        eprintln!("run identity: {e}");
        return 1;
    }
    for cand in cands {
        if let Err(e) = pipe.store.put_candidate(&set_id, cand).await {
            eprintln!("candidate: {e}");
            return 1;
        }
    }
    let decided = if no_decisions {
        // Certified syntax facts publish independently. Reuse only validated
        // model inferences; the gate below protects decision-bearing builds.
        let reused = match chaosbox::decide_cached(
            cands,
            &entities,
            &snap,
            chaosbox_jev::JEV_MODEL_PINNED,
            &mat,
            effective_policy,
            &mut pipe.store,
        )
        .await
        {
            Ok(d) => d,
            Err(e) => {
                eprintln!("decide: {e}");
                return 1;
            }
        };
        // A structural-only build can refresh without model coverage. For
        // decision-bearing or legacy builds retain the conservative exit-4
        // guard when current proposals lack reusable inferences. Relation-local
        // reuse survives unrelated edits, but never an endpoint/policy change.
        // An unreadable active build fails outright (exit 1).
        let active = active_publishes_relations(&pipe.store, repo).await;
        if let Err(error) = &active {
            eprintln!(
                "active build: cannot read the active build's relations ({error}); keeping them untouched and failing"
            );
            return 1;
        }
        if !chaosbox::capture_only_publishable(&active, cands.len(), reused.len()) {
            // Only partial coverage over a relation-bearing build reaches
            // this branch: publishable states (no build yet, or a
            // relationless one) return early above, and unreadable state
            // already failed outright.
            eprintln!(
                "coverage: {} of {} candidate(s) reusable while the active build still publishes relations; keeping it (assess them with `chaosbox run --live-jev`)",
                reused.len(),
                cands.len()
            );
            return 4;
        }
        reused
    } else if live_jev {
        // Fail-closed consent gate first: candidate excerpts contain source
        // text, so live inference needs both the `local` privacy class and
        // the explicit `typesafe-jev` grant. Fixture and snapshot runs never
        // egress source and are unaffected.
        if let Err(e) = effective_policy.require_live_jev() {
            eprintln!("live-jev refused: {e}");
            return 1;
        }
        // Fail fast without credentials: otherwise every decision degrades
        // to Failed and the run exits 0 with an empty graph.
        if chaosbox_jev::JevClient::api_key().is_none() {
            eprintln!("live-jev needs CHAOSBOX_JEV_API_KEY_FILE or TYPESAFE_API_KEY");
            return 1;
        }
        let mut policy = chaosbox_jev::JevPolicy::default();
        if let Some(n) = max_requests {
            policy.max_requests = n;
        }
        if let Some(n) = max_input_tokens {
            policy.max_input_tokens = n;
        }
        if let Some(n) = max_retries {
            policy.max_retries = n;
        }
        // Budget preflight: one uncached candidate costs one Jev request.
        // Fail before spending anything when the budget cannot cover this
        // run (defaults: 200 candidates vs 100 requests), instead of
        // burning the budget and dying at publish on Failed decisions.
        let pending = match chaosbox::uncached_decisions(
            cands,
            &entities,
            &snap,
            chaosbox_jev::JEV_MODEL_PINNED,
            &mat,
            effective_policy,
            &pipe.store,
        )
        .await
        {
            Ok(n) => n,
            Err(e) => {
                eprintln!("budget preflight: {e}");
                return 1;
            }
        };
        let max_requests = usize::try_from(policy.max_requests).unwrap_or(usize::MAX);
        if pending > max_requests {
            eprintln!(
                "live-jev budget: {pending} uncached candidates need one request each but max_requests={}; raise --max-requests or lower --max-candidates (already-cached decisions do not count)",
                policy.max_requests
            );
            // Machine-readable companion to the line above: a batching caller
            // has to tell "this repository does not fit what is left of the
            // allowance, defer it" apart from "this repository can never fit
            // the batch cap, that is a configuration failure".
            eprintln!("budget: pending={pending} allowed={}", policy.max_requests);
            return 1;
        }
        let client = match chaosbox_jev::JevClient::new(policy) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("jev client: {e}");
                return 1;
            }
        };
        let mut responder = LiveResponder::new(client);
        let decided = Pipeline::<S>::decide(
            cands,
            &entities,
            &snap,
            &mut responder,
            chaosbox_jev::JEV_MODEL_PINNED,
            &mat,
            effective_policy,
            &mut pipe.store,
        )
        .await;
        let (requests, tokens) = responder.usage();
        // Record the spend before branching: the caller's single `usage:`
        // line must still say what this run dispatched when it exits here
        // without publishing.
        spend.requests.store(requests, Ordering::Relaxed);
        spend.tokens.store(tokens, Ordering::Relaxed);
        match decided {
            Ok(d) => d,
            Err(e) => {
                eprintln!("decide: {e}");
                return 1;
            }
        }
    } else {
        if !fixture_decisions {
            eprintln!(
                "refusing to publish fixture decisions without --fixture-decisions (fixture graphs are disposable/test-only); pass --live-jev for real decisions"
            );
            return 1;
        }
        let mut responder = FixtureResponder::new(true);
        // Fixture decisions must never masquerade as Jev model output.
        responder.model = chaosbox_jev::FIXTURE_MODEL.into();
        match Pipeline::<S>::decide(
            cands,
            &entities,
            &snap,
            &mut responder,
            chaosbox_jev::FIXTURE_MODEL,
            &mat,
            effective_policy,
            &mut pipe.store,
        )
        .await
        {
            Ok(d) => d,
            Err(e) => {
                eprintln!("decide: {e}");
                return 1;
            }
        }
    };
    // Decision accounting is separate from the uncapped syntax facts above.
    let counts = chaosbox::summarize_outcomes(&decided);
    let n = |k: &str| counts.get(k).copied().unwrap_or(0);
    eprintln!(
        "decisions: accepted={} rejected={} abstained={} negative={} failed={} candidates={}",
        n("accepted"),
        n("rejected"),
        n("abstained"),
        n("negative"),
        n("failed"),
        cands.len()
    );
    if let Some(artifact) = compiler {
        let valid = chaosbox::compiler::load_receipt(artifact)
            .and_then(|receipt| receipt.verify(path).map_err(Into::into));
        if let Err(error) = valid {
            eprintln!("compiler inputs before publication: {error}");
            return 1;
        }
    }
    match pipe
        .build_and_publish(repo, &snap, &ext, &decided, &mat, expected_predecessor)
        .await
    {
        Ok(build) => {
            use std::io::Write as _;
            let mut output = std::io::BufWriter::new(std::io::stdout().lock());
            if let Err(error) = chaosbox::write_export_json(&build, &mut output) {
                eprintln!("export: {error}");
                return 1;
            }
            if let Err(error) = writeln!(output).and_then(|()| output.flush()) {
                eprintln!("export: {error}");
                return 1;
            }
            0
        }
        Err(e) => {
            eprintln!("publish: {e}");
            1
        }
    }
}

/// The `run` command: validate policy, select the backend, run once, and
/// print the live-spend line exactly once on every exit.
pub(super) async fn run_cmd(args: &super::RunArgs) -> i32 {
    use super::{
        backend, typedb_config_from_env, typedb_publication_chain, Backend, MemoryStore,
        TypeDbStore,
    };
    let policy = match super::effective_policy(
        &args.source_paths,
        &args.exclude,
        &args.privacy,
        &args.inference,
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("run failed: {e}");
            return 1;
        }
    };
    // Live spend of this run. Every exit prints it exactly once (via
    // `usage`), so a batching caller can debit what this repository
    // dispatched even when the run dies before it publishes.
    let spend = RunSpend::default();
    let usage = || {
        if args.live_jev {
            // Stable parseable spend line: `canix chaosbox sync`
            // budgets a whole batch against what this run dispatched.
            eprintln!(
                "usage: requests={} input_tokens={}",
                spend.requests.load(Ordering::Relaxed),
                spend.tokens.load(Ordering::Relaxed)
            );
        }
    };
    let (pipe, expected_predecessor) = match backend() {
        Backend::Memory => {
            let code = run_pipeline_with(
                Pipeline::<MemoryStore>::new(),
                &args.path,
                &args.repo,
                args.max_candidates,
                &policy,
                args.live_jev,
                args.fixture_decisions,
                args.no_decisions,
                args.max_requests,
                args.max_input_tokens,
                args.max_retries,
                None,
                &spend,
                args.compiler.as_deref(),
            )
            .await;
            usage();
            return code;
        }
        Backend::Typedb => {
            let config = match typedb_config_from_env() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("typedb config: {e}");
                    usage();
                    return 1;
                }
            };
            let mut store = TypeDbStore::new(config.clone());
            if let Err(e) = Box::pin(store.migrate()).await {
                eprintln!("typedb migrate: {e}");
                usage();
                return 1;
            }
            // Fresh processes start at generation zero: chain off the
            // live active build so repeat runs publish gen+1 with the
            // right predecessor instead of failing the guard.
            match Box::pin(typedb_publication_chain(&config, &args.repo)).await {
                Ok((predecessor, generation)) => (Pipeline { store, generation }, predecessor),
                Err(e) => {
                    eprintln!("typedb publication chain: {e}");
                    usage();
                    return 1;
                }
            }
        }
    };
    let code = run_pipeline_with(
        pipe,
        &args.path,
        &args.repo,
        args.max_candidates,
        &policy,
        args.live_jev,
        args.fixture_decisions,
        args.no_decisions,
        args.max_requests,
        args.max_input_tokens,
        args.max_retries,
        expected_predecessor,
        &spend,
        args.compiler.as_deref(),
    )
    .await;
    usage();
    code
}

/// Graphify's `watch`: re-run whenever the captured sources change. Each
/// check hashes file metadata only; a changed fingerprint is confirmed by
/// the content-addressed snapshot id before publishing, so touches without
/// content changes never republish. Failed runs are reported and the loop
/// continues; Ctrl+C stops it.
pub(super) async fn watch(args: &super::RunArgs, interval_secs: u64) -> i32 {
    let policy = match super::effective_policy(
        &args.source_paths,
        &args.exclude,
        &args.privacy,
        &args.inference,
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("watch failed: {e}");
            return 1;
        }
    };
    let interval = std::time::Duration::from_secs(interval_secs.max(1));
    let (mut fingerprint, mut snapshot) = (None, None);
    loop {
        match chaosbox_extract::source_fingerprint(&args.path, &policy.scope, &policy.exclude) {
            Err(e) => eprintln!("watch: {e}"),
            Ok(current) if fingerprint.as_ref() != Some(&current) => {
                fingerprint = Some(current);
                // ponytail: confirming re-reads changed sources, and the run
                // reads them again; cache the snapshot if watch latency matters.
                match chaosbox_extract::Snapshot::capture_excluding(
                    &args.repo,
                    &args.path,
                    &policy.scope,
                    &policy.exclude,
                ) {
                    Ok(snap) if snapshot.as_ref() != Some(&snap.id) => {
                        eprintln!("watch: sources changed ({}), running", snap.id);
                        let code = Box::pin(run_cmd(args)).await;
                        eprintln!("watch: run exited {code}");
                        snapshot = Some(snap.id);
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("watch: snapshot failed: {e}"),
                }
            }
            Ok(_) => {}
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return 0,
            () = tokio::time::sleep(interval) => {}
        }
    }
}
