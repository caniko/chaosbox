CREATE MIGRATION m2_worker_tasks ONTO m1_chaosbox_init {
    CREATE TYPE default::WorkerTask {
        CREATE REQUIRED PROPERTY task_id: std::str {
            CREATE CONSTRAINT std::exclusive;
        };
        CREATE REQUIRED PROPERTY state: std::str;
        CREATE REQUIRED PROPERTY holder: std::str;
        CREATE REQUIRED PROPERTY expires_at: std::datetime;
        CREATE REQUIRED PROPERTY generation: std::int64;
        CREATE REQUIRED PROPERTY updated_at: std::datetime {
            SET default := std::datetime_current();
        };
    };
};
