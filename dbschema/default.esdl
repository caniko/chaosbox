# Chaosbox Gel schema (authoritative application database).
#
# Design rules enforced here:
# - Typed objects + links everywhere; no giant graph JSON blob.
# - Relationship is a first-class object (Relationship) with typed
#   endpoints, relation type, scope, evidence. Never one endpoint
#   multi-link for all relations.
# - Entity identity separate from occurrences/labels; snapshots never collide.
# - Frequently queried/constrained fields are typed; JSON only for bounded
#   raw provider envelopes / auxiliary metadata.

module default {
    type Repository {
        required name: str {
            constraint exclusive;
        };
    }

    type SourceSnapshot {
        required repo: str;
        required snapshot_id: str {
            constraint exclusive;
        };
        required created_at: datetime {
            default := datetime_current();
        };
    }

    type FileVersion {
        required snapshot: SourceSnapshot;
        required path: str;
        required sha256: str;
        required bytes: int64;
        constraint exclusive on ((.snapshot, .path));
    }

    type SourceSpan {
        required file: str;
        required start_line: int64;
        required start_col: int64;
        required end_line: int64;
        required end_col: int64;
        required byte_start: int64;
        required byte_end: int64;
    }

    type Entity {
        required entity_id: str {
            constraint exclusive;
        };
        required kind: str;
        required repo: str;
        required snapshot: str;
        required file: str;
        required name: str;
        required qualified_name: str;
        required span: SourceSpan;
        multi aliases: str;
    }

    type EntityOccurrence {
        required entity: Entity;
        required file_version: FileVersion;
        required span: SourceSpan;
    }

    type ExtractionRun {
        required run_id: str {
            constraint exclusive;
        };
        required repo: str;
        required snapshot: SourceSnapshot;
        required created_at: datetime {
            default := datetime_current();
        };
    }

    type CandidateSet {
        required set_id: str {
            constraint exclusive;
        };
        required run: ExtractionRun;
        required catalog_digest: str;
        required rubric_version: str;
    }

    type Candidate {
        required candidate_id: str {
            constraint exclusive;
        };
        required candidate_set: CandidateSet;
        required rel_type: str;
        required from_entity: Entity;
        required to_entity: Entity;
        required reason: str;
        required state_excerpt: str;
    }

    type JevAttempt {
        required attempt_id: str {
            constraint exclusive;
        };
        required candidate: Candidate;
        required question_id: str;
        required model_requested: str;
        required model_returned: str;
        required cache_key: str;
        required created_at: datetime {
            default := datetime_current();
        };
        input_tokens: int64;
        http_status: int64;
        error: str;
        # Bounded raw provider envelope only.
        raw_envelope: json;
    }

    type Decision {
        required decision_id: str {
            constraint exclusive;
        };
        required candidate: Candidate;
        required question_id: str;
        required outcome: str;
        required evidence_class: str;
        required model_requested: str;
        required model_returned: str;
        confidence: float64;
        probability: float64;
        constraint exclusive on ((.candidate, .question_id));
    }

    type Evidence {
        required evidence_id: str {
            constraint exclusive;
        };
        required class: str;
        required supports: bool;
        required text: str;
        span: SourceSpan;
        required source_file_version: FileVersion;
    }

    type Claim {
        required claim_id: str {
            constraint exclusive;
        };
        required relationship: Relationship;
        multi supporting: Evidence;
        multi contradicting: Evidence;
        required accepted: bool;
    }

    # First-class relationship: typed endpoints + type + scope + evidence.
    type Relationship {
        required rel_id: str {
            constraint exclusive;
        };
        required rel_type: str;
        required from_entity: Entity;
        required to_entity: Entity;
        required scope: str;
        multi evidence: Evidence;
    }

    type GraphBuild {
        required build_id: str {
            constraint exclusive;
        };
        required repo: str;
        multi snapshots: SourceSnapshot;
        required generation: int64;
        predecessor: GraphBuild;
        required status: str;
        required created_at: datetime {
            default := datetime_current();
        };
    }

    type GraphMembership {
        required build: GraphBuild;
        required entity: Entity;
        constraint exclusive on ((.build, .entity));
    }

    type GraphEdgeMembership {
        required build: GraphBuild;
        required relationship: Relationship;
        constraint exclusive on ((.build, .relationship));
    }

    type ActiveBuildPointer {
        required repo: str {
            constraint exclusive;
        };
        required build: GraphBuild;
        required updated_at: datetime {
            default := datetime_current();
        };
    }

    type Community {
        required community_id: str {
            constraint exclusive;
        };
        required build: GraphBuild;
        # Source-derived or deterministic label only; never generative.
        required label: str;
        required version: int64;
    }

    type CommunityMembership {
        required community: Community;
        required entity: Entity;
        constraint exclusive on ((.community, .entity));
    }

    type Hyperedge {
        required hyperedge_id: str {
            constraint exclusive;
        };
        required build: GraphBuild;
        required label: str;
    }

    type HyperedgeMembership {
        required hyperedge: Hyperedge;
        required entity: Entity;
        constraint exclusive on ((.hyperedge, .entity));
    }

    # Durable worker task with a lease: a crashed holder's task becomes
    # reclaimable after expiry, but the generation guard means a stale
    # holder can never overwrite newer results.
    type WorkerTask {
        required task_id: str {
            constraint exclusive;
        };
        required state: str;
        required holder: str;
        required expires_at: datetime;
        required generation: int64;
        required updated_at: datetime {
            default := datetime_current();
        };
    }
}
