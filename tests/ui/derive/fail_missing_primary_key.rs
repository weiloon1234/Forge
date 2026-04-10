#![allow(unused_imports)]

use forge::prelude::*;

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "users")]
struct User {
    sale_id: i64,
}

fn main() {}
