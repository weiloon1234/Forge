#![allow(unused_imports)]

use forge::prelude::*;

#[derive(Clone, Debug, PartialEq, forge::Projection)]
struct UserRow {
    tags: Loaded<Vec<String>>,
}

fn main() {}
