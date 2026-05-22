use serde::Serialize;

#[derive(Serialize, ts_rs::TS, forge::ApiSchema)]
#[ts(export)]
struct BadExport {
    value: String,
}

fn main() {}
