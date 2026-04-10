#![allow(unused_imports)]

use forge::prelude::*;

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "users")]
struct User {
    id: i64,
    #[forge(column = "email")]
    primary_email: String,
    email: String,
}

fn main() {}
