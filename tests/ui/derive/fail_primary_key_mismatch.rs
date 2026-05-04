#![allow(unused_imports)]

use forge::prelude::*;

#[derive(forge::Model)]
#[forge(table = "users", primary_key = "user_id")]
struct User {
    id: i64,
    email: String,
}

fn main() {}
