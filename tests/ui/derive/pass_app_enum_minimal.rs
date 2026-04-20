use forge::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, forge::AppEnum)]
enum MinimalStatus {
    Pending,
    Completed,
}

fn main() {
    let _ = MinimalStatus::id();
    let _ = MinimalStatus::options();
    let _ = MinimalStatus::Pending.label_key();
}
