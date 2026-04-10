use forge::prelude::*;

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "merchants")]
struct Merchant {
    id: i64,
    tags: Loaded<Vec<Tag>>,
    tag_count: Loaded<i64>,
}

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "tags")]
struct Tag {
    id: i64,
    name: String,
    link: Loaded<TagLink>,
}

#[derive(Clone, Debug, PartialEq, forge::Projection)]
struct TagLink {
    #[forge(source = "role")]
    role: String,
}

impl Merchant {
    fn tags() -> ManyToManyDef<Self, Tag, ()> {
        many_to_many(
            "tags",
            Self::ID,
            "merchant_tags",
            "merchant_id",
            "tag_id",
            Tag::ID,
            |merchant| merchant.id,
            |merchant, tags| merchant.tags = Loaded::new(tags),
        )
    }

    fn tags_with_pivot() -> ManyToManyDef<Self, Tag, TagLink> {
        Self::tags().with_pivot(TagLink::projection_meta(), |tag, link| {
            tag.link = Loaded::new(link)
        })
    }

    fn tag_count() -> RelationAggregateDef<Self, i64> {
        Self::tags().count(|merchant, count| merchant.tag_count = Loaded::new(count))
    }
}

fn main() -> Result<()> {
    let query = Merchant::query()
        .with_aggregate(Merchant::tag_count())
        .with_many_to_many(Merchant::tags_with_pivot());

    println!("{:?}", query.ast());
    Ok(())
}
