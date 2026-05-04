use forge::ForgeId;

#[derive(ForgeId)]
#[forge(id = forge::GuardId, prefix = "admin")]
enum Guard {
    #[forge(value = "api")]
    Api,
}

fn main() {}
