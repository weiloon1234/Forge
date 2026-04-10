use forge::prelude::*;

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "users")]
struct User {
    id: i64,
    merchants: Loaded<Vec<Merchant>>,
    merchant_count: Loaded<i64>,
}

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "merchants")]
struct Merchant {
    id: i64,
    user_id: i64,
    orders: Loaded<Vec<Order>>,
    order_total: Loaded<Option<i64>>,
}

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "orders")]
struct Order {
    id: i64,
    merchant_id: i64,
    total: i64,
    items: Loaded<Vec<OrderItem>>,
}

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "order_items")]
struct OrderItem {
    id: i64,
    order_id: i64,
    product_id: i64,
    product: Loaded<Option<Product>>,
}

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "products")]
struct Product {
    id: i64,
}

impl User {
    fn merchants() -> RelationDef<Self, Merchant> {
        has_many(
            "merchants",
            Self::ID,
            Merchant::USER_ID,
            |user| user.id,
            |user, merchants| user.merchants = Loaded::new(merchants),
        )
    }

    fn merchant_count() -> RelationAggregateDef<Self, i64> {
        Self::merchants().count(|user, count| user.merchant_count = Loaded::new(count))
    }
}

impl Merchant {
    fn orders() -> RelationDef<Self, Order> {
        has_many(
            "orders",
            Self::ID,
            Order::MERCHANT_ID,
            |merchant| merchant.id,
            |merchant, orders| merchant.orders = Loaded::new(orders),
        )
    }

    fn order_total() -> RelationAggregateDef<Self, Option<i64>> {
        Self::orders().sum(Order::TOTAL, |merchant, total| {
            merchant.order_total = Loaded::new(total)
        })
    }
}

impl Order {
    fn items() -> RelationDef<Self, OrderItem> {
        has_many(
            "items",
            Self::ID,
            OrderItem::ORDER_ID,
            |order| order.id,
            |order, items| order.items = Loaded::new(items),
        )
    }
}

impl OrderItem {
    fn product() -> RelationDef<Self, Product> {
        belongs_to(
            "product",
            Self::PRODUCT_ID,
            Product::ID,
            |item| Some(item.product_id),
            |item, product| item.product = Loaded::new(product),
        )
    }
}

fn main() -> Result<()> {
    let query = User::query().with_aggregate(User::merchant_count()).with(
        User::merchants()
            .with_aggregate(Merchant::order_total())
            .with(Merchant::orders().with(Order::items().with(OrderItem::product()))),
    );

    println!("{:?}", query.ast());
    Ok(())
}
