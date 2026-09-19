CREATE MIGRATION m3_decision_cache_key ONTO m2_worker_tasks {
    ALTER TYPE default::Decision {
        CREATE REQUIRED PROPERTY cache_key: std::str;
    };
};
