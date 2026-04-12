use forge::prelude::*;

#[derive(forge::Model)]
#[forge(model = "posts", timestamps = true)]
struct Post {
    id: ModelId<Post>,
    title: String,
}

fn main() {}
