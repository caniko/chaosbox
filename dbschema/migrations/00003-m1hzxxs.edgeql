CREATE MIGRATION m1hzxxsspfr3msambqsztuyxg4b63khfwpjg7pmhsqbd4bprndljya
    ONTO m12njzrcvjntb32dcaaa4tl6ac34fenx7qczc3skbufpfimsp4zgeq
{
  CREATE TYPE default::SourceSnapshot {
      CREATE REQUIRED PROPERTY created_at: std::datetime {
          SET default := (std::datetime_current());
      };
      CREATE REQUIRED PROPERTY repo: std::str;
      CREATE REQUIRED PROPERTY snapshot_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
  };
  CREATE TYPE default::GraphBuild {
      CREATE LINK predecessor: default::GraphBuild;
      CREATE MULTI LINK snapshots: default::SourceSnapshot;
      CREATE REQUIRED PROPERTY build_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY created_at: std::datetime {
          SET default := (std::datetime_current());
      };
      CREATE REQUIRED PROPERTY generation: std::int64;
      CREATE REQUIRED PROPERTY repo: std::str;
      CREATE REQUIRED PROPERTY status: std::str;
  };
  CREATE TYPE default::ActiveBuildPointer {
      CREATE REQUIRED LINK build: default::GraphBuild;
      CREATE REQUIRED PROPERTY repo: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY updated_at: std::datetime {
          SET default := (std::datetime_current());
      };
  };
  CREATE TYPE default::ExtractionRun {
      CREATE REQUIRED LINK snapshot: default::SourceSnapshot;
      CREATE REQUIRED PROPERTY created_at: std::datetime {
          SET default := (std::datetime_current());
      };
      CREATE REQUIRED PROPERTY repo: std::str;
      CREATE REQUIRED PROPERTY run_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
  };
  CREATE TYPE default::CandidateSet {
      CREATE REQUIRED LINK run: default::ExtractionRun;
      CREATE REQUIRED PROPERTY catalog_digest: std::str;
      CREATE REQUIRED PROPERTY rubric_version: std::str;
      CREATE REQUIRED PROPERTY set_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
  };
  CREATE TYPE default::SourceSpan {
      CREATE REQUIRED PROPERTY byte_end: std::int64;
      CREATE REQUIRED PROPERTY byte_start: std::int64;
      CREATE REQUIRED PROPERTY end_col: std::int64;
      CREATE REQUIRED PROPERTY end_line: std::int64;
      CREATE REQUIRED PROPERTY file: std::str;
      CREATE REQUIRED PROPERTY start_col: std::int64;
      CREATE REQUIRED PROPERTY start_line: std::int64;
  };
  CREATE TYPE default::Entity {
      CREATE REQUIRED LINK span: default::SourceSpan;
      CREATE MULTI PROPERTY aliases: std::str;
      CREATE REQUIRED PROPERTY entity_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY file: std::str;
      CREATE REQUIRED PROPERTY kind: std::str;
      CREATE REQUIRED PROPERTY name: std::str;
      CREATE REQUIRED PROPERTY qualified_name: std::str;
      CREATE REQUIRED PROPERTY repo: std::str;
      CREATE REQUIRED PROPERTY snapshot: std::str;
  };
  CREATE TYPE default::Candidate {
      CREATE REQUIRED LINK candidate_set: default::CandidateSet;
      CREATE REQUIRED LINK from_entity: default::Entity;
      CREATE REQUIRED LINK to_entity: default::Entity;
      CREATE REQUIRED PROPERTY candidate_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY reason: std::str;
      CREATE REQUIRED PROPERTY rel_type: std::str;
      CREATE REQUIRED PROPERTY state_excerpt: std::str;
  };
  CREATE TYPE default::Decision {
      CREATE REQUIRED LINK candidate: default::Candidate;
      CREATE REQUIRED PROPERTY question_id: std::str;
      CREATE CONSTRAINT std::exclusive ON ((.candidate, .question_id));
      CREATE REQUIRED PROPERTY cache_key: std::str;
      CREATE PROPERTY confidence: std::float64;
      CREATE REQUIRED PROPERTY decision_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY evidence_class: std::str;
      CREATE REQUIRED PROPERTY model_requested: std::str;
      CREATE REQUIRED PROPERTY model_returned: std::str;
      CREATE REQUIRED PROPERTY outcome: std::str;
      CREATE PROPERTY probability: std::float64;
  };
  CREATE TYPE default::JevAttempt {
      CREATE REQUIRED LINK candidate: default::Candidate;
      CREATE REQUIRED PROPERTY attempt_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY cache_key: std::str;
      CREATE REQUIRED PROPERTY created_at: std::datetime {
          SET default := (std::datetime_current());
      };
      CREATE PROPERTY error: std::str;
      CREATE PROPERTY http_status: std::int64;
      CREATE PROPERTY input_tokens: std::int64;
      CREATE REQUIRED PROPERTY model_requested: std::str;
      CREATE REQUIRED PROPERTY model_returned: std::str;
      CREATE REQUIRED PROPERTY question_id: std::str;
      CREATE PROPERTY raw_envelope: std::json;
  };
  CREATE TYPE default::FileVersion {
      CREATE REQUIRED LINK snapshot: default::SourceSnapshot;
      CREATE REQUIRED PROPERTY path: std::str;
      CREATE CONSTRAINT std::exclusive ON ((.snapshot, .path));
      CREATE REQUIRED PROPERTY bytes: std::int64;
      CREATE REQUIRED PROPERTY sha256: std::str;
  };
  CREATE TYPE default::Evidence {
      CREATE REQUIRED LINK source_file_version: default::FileVersion;
      CREATE LINK span: default::SourceSpan;
      CREATE REQUIRED PROPERTY class: std::str;
      CREATE REQUIRED PROPERTY evidence_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY supports: std::bool;
      CREATE REQUIRED PROPERTY text: std::str;
  };
  CREATE TYPE default::Relationship {
      CREATE REQUIRED LINK from_entity: default::Entity;
      CREATE REQUIRED LINK to_entity: default::Entity;
      CREATE MULTI LINK evidence: default::Evidence;
      CREATE REQUIRED PROPERTY rel_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY rel_type: std::str;
      CREATE REQUIRED PROPERTY scope: std::str;
  };
  CREATE TYPE default::Claim {
      CREATE MULTI LINK contradicting: default::Evidence;
      CREATE REQUIRED LINK relationship: default::Relationship;
      CREATE MULTI LINK supporting: default::Evidence;
      CREATE REQUIRED PROPERTY accepted: std::bool;
      CREATE REQUIRED PROPERTY claim_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
  };
  CREATE TYPE default::Community {
      CREATE REQUIRED LINK build: default::GraphBuild;
      CREATE REQUIRED PROPERTY community_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY label: std::str;
      CREATE REQUIRED PROPERTY version: std::int64;
  };
  CREATE TYPE default::CommunityMembership {
      CREATE REQUIRED LINK community: default::Community;
      CREATE REQUIRED LINK entity: default::Entity;
      CREATE CONSTRAINT std::exclusive ON ((.community, .entity));
  };
  CREATE TYPE default::GraphMembership {
      CREATE REQUIRED LINK build: default::GraphBuild;
      CREATE REQUIRED LINK entity: default::Entity;
      CREATE CONSTRAINT std::exclusive ON ((.build, .entity));
  };
  CREATE TYPE default::Hyperedge {
      CREATE REQUIRED LINK build: default::GraphBuild;
      CREATE REQUIRED PROPERTY hyperedge_id: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
      CREATE REQUIRED PROPERTY label: std::str;
  };
  CREATE TYPE default::HyperedgeMembership {
      CREATE REQUIRED LINK entity: default::Entity;
      CREATE REQUIRED LINK hyperedge: default::Hyperedge;
      CREATE CONSTRAINT std::exclusive ON ((.hyperedge, .entity));
  };
  CREATE TYPE default::EntityOccurrence {
      CREATE REQUIRED LINK entity: default::Entity;
      CREATE REQUIRED LINK file_version: default::FileVersion;
      CREATE REQUIRED LINK span: default::SourceSpan;
  };
  CREATE TYPE default::GraphEdgeMembership {
      CREATE REQUIRED LINK build: default::GraphBuild;
      CREATE REQUIRED LINK relationship: default::Relationship;
      CREATE CONSTRAINT std::exclusive ON ((.build, .relationship));
  };
  CREATE TYPE default::Repository {
      CREATE REQUIRED PROPERTY name: std::str {
          CREATE CONSTRAINT std::exclusive;
      };
  };
};
