use forge::ForgeId;

#[derive(ForgeId)]
#[forge(id = forge::GuardId)]
enum Guard {
    #[forge(value = "api")]
    Api,
    #[forge(value = "api")]
    Admin,
}

fn main() {}
