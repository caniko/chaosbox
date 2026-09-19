CREATE MIGRATION m1eoizmzdgtcju5eskmicz5ob5ijy743xyh35vkuzakx2lbdmktkjq ONTO m12njzrcvjntb32dcaaa4tl6ac34fenx7qczc3skbufpfimsp4zgeq {
    ALTER TYPE default::Decision {
        CREATE REQUIRED PROPERTY cache_key: std::str;
    };
};
