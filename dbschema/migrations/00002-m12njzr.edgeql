CREATE MIGRATION m12njzrcvjntb32dcaaa4tl6ac34fenx7qczc3skbufpfimsp4zgeq ONTO m1tjyzfl33vvzwjd5izo5nyp4zdsekyvxpdm7zhtt5ufmqjzczopdq {
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
