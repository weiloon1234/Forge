#![allow(unused_imports)]

use forge::prelude::*;

#[derive(forge::Model)]
#[forge(model = "users", primary_key_strategy = "manual")]
struct User {
    id: i64,
    #[forge(column = "email")]
    primary_email: String,
    email: String,
}

fn main() {}
