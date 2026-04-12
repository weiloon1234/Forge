#![allow(unused_imports)]

use forge::prelude::*;

#[derive(forge::Model)]
#[forge(model = "users")]
struct User {
    sale_id: i64,
}

fn main() {}
