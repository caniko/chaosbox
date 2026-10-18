pub fn legacy() -> &'static str {
    api_v1::context()
}

pub fn current() -> &'static str {
    api_v2::context()
}

pub fn unavailable() -> &'static str {
    missing_api::context()
}

pub fn restricted() -> &'static str {
    private_api::context()
}
