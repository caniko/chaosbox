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
    live_jev: bool,
    fixture_decisions: bool,
    no_decisions: bool,
    max_requests: Option<u32>,
    max_input_tokens: Option<u64>,
    max_retries: Option<u32>,
    expected_predecessor: Option<String>,
    spend: &RunSpend,
) -> i32 {
    let (snap, ext, cat) = match Pipeline::<S>::snapshot_extract(repo, path, max_candidates) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("extract: {e}");
            return 1;
        }
    };
    let cands = &cat.candidates;
    // Truncation must be observable: report what the cap selected and
    // omitted before any decision is made.
    eprintln!(
        "candidates: selected={} cap={} omitted={}",
        cands.len(),
        cat.cap,
        serde_json::to_string(&cat.omitted).unwrap(),
    );
    let entities: BTreeMap<_, _> = ext
        .entities
        .iter()
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
        // No live inference and no fixture accept-all: republish the
        // decisions an earlier run already paid for and skip the rest, so
        // an entities-only refresh can never swing the active pointer to a
        // build that silently drops published relations.
        let reused = match chaosbox::decide_cached(
            cands,
            &entities,
            chaosbox_jev::JEV_MODEL_PINNED,
            &mat,
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
        // Cache identity carries the repository snapshot and the whole
        // catalog, so an ordinary source edit leaves nothing reusable.
        // Publishing that graph would swing the active pointer onto a build
        // with fewer relations than the one consumers are querying today;
        // keep it instead and report the pending work. Exit 4 means "kept
        // the previous build (or could not read it), spent nothing", which
        // callers defer on rather than retry with backoff. The one exception
        // is a repository with no active build at all: there is nothing to
        // keep, so the first capture-only publish goes ahead and bootstraps
        // the query view without spending anything.
        let active = active_publishes_relations(&pipe.store, repo).await;
        if !chaosbox::capture_only_publishable(active, cands.len(), reused.len()) {
            // Publishable states (no build yet, or a relationless one) never
            // reach this branch: what remains is partial coverage over a
            // relation-bearing build, or state too unreadable to risk.
            if active.is_err() {
                eprintln!(
                    "coverage: cannot read the active build's relations; keeping them untouched (Chaosbox never replaces a build it cannot account for)"
                );
            } else {
                eprintln!(
                    "coverage: {} of {} candidate(s) reusable while the active build still publishes relations; keeping it (assess them with `chaosbox run --live-jev`)",
                    reused.len(),
                    cands.len()
                );
            }
            return 4;
        }
        reused
    } else if live_jev {
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
            chaosbox_jev::JEV_MODEL_PINNED,
            &mat,
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
            &mut responder,
            chaosbox_jev::JEV_MODEL_PINNED,
            &mat,
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
        responder.model = "fixture-test".into();
        match Pipeline::<S>::decide(
            cands,
            &entities,
            &mut responder,
            "fixture-test",
            &mat,
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
    // Operator visibility: structural vs semantic coverage is a follow-up;
    // today every candidate consumes the Jev budget, so report the outcome
    // mix before publication (a failed batch refuses to publish below).
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
    match pipe
        .build_and_publish(repo, &snap, &ext, &decided, &mat, expected_predecessor)
        .await
    {
        Ok(build) => {
            let v = chaosbox::export_json(&build);
            println!("{}", serde_json::to_string(&v).unwrap());
            0
        }
        Err(e) => {
            eprintln!("publish: {e}");
            1
        }
    }
}
