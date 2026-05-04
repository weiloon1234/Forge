use forge::prelude::*;

#[derive(forge::Model)]
#[forge(table = "posts", timestamps = true)]
struct Post {
    id: ModelId<Post>,
    title: String,
}

fn main() {}
