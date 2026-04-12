use forge::prelude::*;

#[derive(forge::Model)]
#[forge(model = "posts", timestamps = true)]
struct Post {
    id: ModelId<Post>,
    title: String,
    created_at: String,
    updated_at: DateTime,
}

fn main() {}
