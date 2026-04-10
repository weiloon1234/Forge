#![allow(unused_imports)]

use forge::prelude::*;

#[derive(Clone, Debug, PartialEq, forge::Projection)]
struct UserRow {
    email: String,
    #[forge(alias = "email")]
    secondary_email: String,
}

fn main() {}
