use std::collections::{BTreeSet, HashMap};
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;

use crate::foundation::{Error, Result};

use super::ast::{
    AggregateNode, ColumnRef, ComparisonOp, Condition, DbValue, Expr, JoinKind, JoinNode, QueryAst,
    RelationKind, RelationNode, SelectItem, SelectNode, TableRef,
};
use super::compiler::PostgresCompiler;
use super::extensions::{register_model_records, AnyModelExtension};
use super::model::{Column, FromDbValue, Model, ToDbValue};
use super::projection::ProjectionMeta;
use super::query::ModelQuery;
use super::runtime::{DbRecord, QueryExecutor};

const RELATION_GROUP_KEY_ALIAS: &str = "__forge_relation_key";
const RELATION_AGGREGATE_ALIAS: &str = "__forge_relation_aggregate";
const PIVOT_ALIAS_PREFIX: &str = "__forge_pivot_";

#[async_trait]
pub trait RelationLoader<From: Send>: Send + Sync {
    /// Returns the relation node metadata.
    fn node(&self) -> RelationNode;
    /// Batch-loads related models onto the given parent slice in-place.
    async fn load(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()>;

    /// Like `load`, but skips parents where the relation is already loaded.
    /// Falls back to `load` if no `is_loaded` checker is configured.
    async fn load_missing(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()> {
        self.load(executor, parents).await
    }
}

/// Type-erased relation loader: `Arc<dyn RelationLoader<M>>`.
pub type AnyRelation<M> = Arc<dyn RelationLoader<M>>;

#[async_trait]
pub(crate) trait RelationAggregateLoader<From>: Send + Sync {
    fn node(&self) -> RelationNode;
    async fn load(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()>;
}

pub(crate) type AnyRelationAggregate<M> = Arc<dyn RelationAggregateLoader<M>>;

type ParentKeyFn<From> = dyn Fn(&From) -> Option<DbValue> + Send + Sync;
type IsLoadedFn<From> = dyn Fn(&From) -> bool + Send + Sync;
type AttachManyFn<From, To> = dyn Fn(&mut From, Vec<To>) + Send + Sync;
type AttachOneFn<From, To> = dyn Fn(&mut From, Option<To>) + Send + Sync;
type AttachAggregateFn<From, Value> = dyn Fn(&mut From, Value) + Send + Sync;
type PivotAttachFn<To, Pivot> = dyn Fn(&mut To, Pivot) + Send + Sync;

trait PivotAttacher<To>: Send + Sync {
    fn select_items(&self, table_name: &str) -> Result<Vec<SelectItem>>;
    fn attach(&self, record: &DbRecord, child: &mut To) -> Result<()>;
}

type AnyPivotAttacher<To> = Arc<dyn PivotAttacher<To>>;

#[derive(Clone)]
pub struct RelationDef<From, To: 'static> {
    name: String,
    kind: RelationKind,
    parent_column: ColumnRef,
    target_column: ColumnRef,
    target_table: &'static super::model::TableMeta<To>,
    parent_key: Arc<ParentKeyFn<From>>,
    attach: RelationAttach<From, To>,
    is_loaded: Option<Arc<IsLoadedFn<From>>>,
    filter: Option<Condition>,
    children: Vec<AnyRelation<To>>,
    child_extensions: Vec<AnyModelExtension<To>>,
    child_aggregates: Vec<AnyRelationAggregate<To>>,
}

#[derive(Clone)]
enum RelationAttach<From, To> {
    Many(Arc<AttachManyFn<From, To>>),
    One(Arc<AttachOneFn<From, To>>),
}

#[derive(Clone)]
pub struct ManyToManyDef<From, To: 'static, Pivot: 'static = ()> {
    name: String,
    parent_column: ColumnRef,
    pivot_table: TableRef,
    pivot_parent_column: ColumnRef,
    pivot_target_column: ColumnRef,
    target_column: ColumnRef,
    target_table: &'static super::model::TableMeta<To>,
    parent_key: Arc<ParentKeyFn<From>>,
    attach: Arc<AttachManyFn<From, To>>,
    is_loaded: Option<Arc<IsLoadedFn<From>>>,
    filter: Option<Condition>,
    children: Vec<AnyRelation<To>>,
    child_extensions: Vec<AnyModelExtension<To>>,
    child_aggregates: Vec<AnyRelationAggregate<To>>,
    pivot_attacher: Option<AnyPivotAttacher<To>>,
    _pivot: PhantomData<fn() -> Pivot>,
}

#[derive(Clone)]
pub struct RelationAggregateDef<From, Value: 'static> {
    loader: AnyRelationAggregate<From>,
    _marker: PhantomData<fn() -> Value>,
}

impl<From, Value: 'static> RelationAggregateDef<From, Value> {
    pub(crate) fn new(loader: AnyRelationAggregate<From>) -> Self {
        Self {
            loader,
            _marker: PhantomData,
        }
    }

    pub(crate) fn node(&self) -> RelationNode {
        self.loader.node()
    }

    pub(crate) fn into_loader(self) -> AnyRelationAggregate<From> {
        self.loader
    }
}

#[derive(Clone)]
enum AggregateKind {
    CountAll,
    CountDistinct(ColumnRef),
    Sum(ColumnRef),
    Avg(ColumnRef),
    Min(ColumnRef),
    Max(ColumnRef),
}

impl AggregateKind {
    fn node(self) -> AggregateNode {
        match self {
            Self::CountAll => AggregateNode::count_all(RELATION_AGGREGATE_ALIAS),
            Self::CountDistinct(column) => {
                AggregateNode::count_distinct(Expr::column(column), RELATION_AGGREGATE_ALIAS)
            }
            Self::Sum(column) => AggregateNode::sum(Expr::column(column), RELATION_AGGREGATE_ALIAS),
            Self::Avg(column) => AggregateNode::avg(Expr::column(column), RELATION_AGGREGATE_ALIAS),
            Self::Min(column) => AggregateNode::min(Expr::column(column), RELATION_AGGREGATE_ALIAS),
            Self::Max(column) => AggregateNode::max(Expr::column(column), RELATION_AGGREGATE_ALIAS),
        }
    }
}

#[derive(Clone)]
struct TypedPivotAttacher<To, Pivot: 'static> {
    meta: &'static ProjectionMeta<Pivot>,
    attach: Arc<PivotAttachFn<To, Pivot>>,
}

impl<To, Pivot> PivotAttacher<To> for TypedPivotAttacher<To, Pivot>
where
    Pivot: Clone + Send + Sync + 'static,
    To: Send + Sync + 'static,
{
    fn select_items(&self, table_name: &str) -> Result<Vec<SelectItem>> {
        self.meta
            .fields()
            .iter()
            .map(|field| {
                let source_column = field.source_column.ok_or_else(|| {
                    Error::message("pivot projection field requires a source column")
                })?;
                Ok(
                    SelectItem::new(ColumnRef::new(table_name, source_column).typed(field.db_type))
                        .aliased(format!("{PIVOT_ALIAS_PREFIX}{}", field.alias)),
                )
            })
            .collect()
    }

    fn attach(&self, record: &DbRecord, child: &mut To) -> Result<()> {
        let mut pivot_record = DbRecord::new();
        for field in self.meta.fields() {
            let key = format!("{PIVOT_ALIAS_PREFIX}{}", field.alias);
            let value = record
                .get(&key)
                .ok_or_else(|| Error::message(format!("missing pivot field `{key}` in record")))?;
            pivot_record.insert(field.alias.to_string(), value.clone());
        }
        let pivot = self.meta.hydrate_record(&pivot_record)?;
        (self.attach)(child, pivot);
        Ok(())
    }
}

impl<From, To> RelationDef<From, To>
where
    From: Model,
    To: Model,
{
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with<Child>(mut self, child: RelationDef<To, Child>) -> Self
    where
        Child: Model,
    {
        self.children.push(Arc::new(child));
        self
    }

    pub fn with_many_to_many<Child, Pivot>(mut self, child: ManyToManyDef<To, Child, Pivot>) -> Self
    where
        Child: Model,
        Pivot: Clone + Send + Sync + 'static,
    {
        self.children.push(Arc::new(child));
        self
    }

    pub fn with_attachments(mut self, collection: impl Into<String>) -> Self
    where
        To: crate::attachments::HasAttachments,
    {
        self.child_extensions
            .push(crate::attachments::attachment_extension_loader(
                collection.into(),
            ));
        self
    }

    pub fn with_translated_field(mut self, field: impl Into<String>) -> Self
    where
        To: crate::translations::HasTranslations,
    {
        self.child_extensions
            .push(crate::translations::translated_field_extension_loader(
                field.into(),
            ));
        self
    }

    pub fn with_translations_for(mut self, locale: impl Into<String>) -> Self
    where
        To: crate::translations::HasTranslations,
    {
        self.child_extensions
            .push(crate::translations::translations_for_extension_loader(
                locale.into(),
            ));
        self
    }

    pub fn with_all_translations(mut self) -> Self
    where
        To: crate::translations::HasTranslations,
    {
        self.child_extensions
            .push(crate::translations::all_translations_extension_loader());
        self
    }

    pub fn with_aggregate<Value>(mut self, aggregate: RelationAggregateDef<To, Value>) -> Self {
        self.child_aggregates.push(aggregate.into_loader());
        self
    }

    pub fn where_(mut self, condition: Condition) -> Self {
        self.filter = merge_condition(self.filter.take(), condition);
        self
    }

    pub fn is_loaded(mut self, f: impl Fn(&From) -> bool + Send + Sync + 'static) -> Self {
        self.is_loaded = Some(Arc::new(f));
        self
    }

    pub fn count(self, attach: fn(&mut From, i64)) -> RelationAggregateDef<From, i64> {
        RelationAggregateDef::new(Arc::new(ScalarRelationAggregate {
            relation: self,
            kind: AggregateKind::CountAll,
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn count_distinct<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, i64),
    ) -> RelationAggregateDef<From, i64> {
        RelationAggregateDef::new(Arc::new(ScalarRelationAggregate {
            relation: self,
            kind: AggregateKind::CountDistinct(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn sum<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, Option<Value>),
    ) -> RelationAggregateDef<From, Option<Value>>
    where
        Value: ToDbValue + FromDbValue + Send + Sync + 'static,
    {
        RelationAggregateDef::new(Arc::new(ScalarRelationAggregate {
            relation: self,
            kind: AggregateKind::Sum(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn avg<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, Option<Value>),
    ) -> RelationAggregateDef<From, Option<Value>>
    where
        Value: ToDbValue + FromDbValue + Send + Sync + 'static,
    {
        RelationAggregateDef::new(Arc::new(ScalarRelationAggregate {
            relation: self,
            kind: AggregateKind::Avg(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn min<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, Option<Value>),
    ) -> RelationAggregateDef<From, Option<Value>>
    where
        Value: ToDbValue + FromDbValue + Send + Sync + 'static,
    {
        RelationAggregateDef::new(Arc::new(ScalarRelationAggregate {
            relation: self,
            kind: AggregateKind::Min(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn max<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, Option<Value>),
    ) -> RelationAggregateDef<From, Option<Value>>
    where
        Value: ToDbValue + FromDbValue + Send + Sync + 'static,
    {
        RelationAggregateDef::new(Arc::new(ScalarRelationAggregate {
            relation: self,
            kind: AggregateKind::Max(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn node(&self) -> RelationNode {
        RelationNode {
            name: self.name.clone(),
            kind: self.kind,
            target: self.target_table.table_ref(),
            local_key: self.parent_column.clone(),
            foreign_key: self.target_column.clone(),
            pivot: None,
            filters: self.filter.clone(),
            children: self
                .children
                .iter()
                .map(|child| child.node())
                .chain(
                    self.child_aggregates
                        .iter()
                        .map(|aggregate| aggregate.node()),
                )
                .collect(),
            aggregates: Vec::new(),
        }
    }

    pub(crate) fn scoped_with_filter(mut self, filter: Option<Condition>) -> Self {
        if let Some(filter) = filter {
            self.filter = merge_condition(self.filter.take(), filter);
        }
        self
    }

    pub(crate) fn exists_condition(&self) -> Condition {
        let mut exists_select = SelectNode::from(self.target_table.table_ref());
        exists_select.columns = vec![SelectItem::new(Expr::raw("1"))];
        let condition = Condition::compare(
            Expr::column(self.target_column.clone()),
            ComparisonOp::Eq,
            Expr::column(self.parent_column.clone()),
        );
        exists_select.condition = Some(match self.filter.clone() {
            Some(filter) => Condition::and([filter, condition]),
            None => condition,
        });
        Condition::exists(QueryAst::select(exists_select))
    }
}

impl<From, To, Pivot> ManyToManyDef<From, To, Pivot>
where
    From: Model,
    To: Model,
    Pivot: Clone + Send + Sync + 'static,
{
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with<Child>(mut self, child: RelationDef<To, Child>) -> Self
    where
        Child: Model,
    {
        self.children.push(Arc::new(child));
        self
    }

    pub fn with_many_to_many<Child, ChildPivot>(
        mut self,
        child: ManyToManyDef<To, Child, ChildPivot>,
    ) -> Self
    where
        Child: Model,
        ChildPivot: Clone + Send + Sync + 'static,
    {
        self.children.push(Arc::new(child));
        self
    }

    pub fn with_attachments(mut self, collection: impl Into<String>) -> Self
    where
        To: crate::attachments::HasAttachments,
    {
        self.child_extensions
            .push(crate::attachments::attachment_extension_loader(
                collection.into(),
            ));
        self
    }

    pub fn with_translated_field(mut self, field: impl Into<String>) -> Self
    where
        To: crate::translations::HasTranslations,
    {
        self.child_extensions
            .push(crate::translations::translated_field_extension_loader(
                field.into(),
            ));
        self
    }

    pub fn with_translations_for(mut self, locale: impl Into<String>) -> Self
    where
        To: crate::translations::HasTranslations,
    {
        self.child_extensions
            .push(crate::translations::translations_for_extension_loader(
                locale.into(),
            ));
        self
    }

    pub fn with_all_translations(mut self) -> Self
    where
        To: crate::translations::HasTranslations,
    {
        self.child_extensions
            .push(crate::translations::all_translations_extension_loader());
        self
    }

    pub fn with_aggregate<Value>(mut self, aggregate: RelationAggregateDef<To, Value>) -> Self {
        self.child_aggregates.push(aggregate.into_loader());
        self
    }

    pub fn where_(mut self, condition: Condition) -> Self {
        self.filter = merge_condition(self.filter.take(), condition);
        self
    }

    pub fn is_loaded(mut self, f: impl Fn(&From) -> bool + Send + Sync + 'static) -> Self {
        self.is_loaded = Some(Arc::new(f));
        self
    }

    pub fn with_pivot<NewPivot>(
        self,
        meta: &'static ProjectionMeta<NewPivot>,
        attach: fn(&mut To, NewPivot),
    ) -> ManyToManyDef<From, To, NewPivot>
    where
        NewPivot: Clone + Send + Sync + 'static,
    {
        ManyToManyDef {
            name: self.name,
            parent_column: self.parent_column,
            pivot_table: self.pivot_table,
            pivot_parent_column: self.pivot_parent_column,
            pivot_target_column: self.pivot_target_column,
            target_column: self.target_column,
            target_table: self.target_table,
            parent_key: self.parent_key,
            attach: self.attach,
            is_loaded: self.is_loaded,
            filter: self.filter,
            children: self.children,
            child_extensions: self.child_extensions,
            child_aggregates: self.child_aggregates,
            pivot_attacher: Some(Arc::new(TypedPivotAttacher {
                meta,
                attach: Arc::new(attach),
            })),
            _pivot: PhantomData,
        }
    }

    pub fn count(self, attach: fn(&mut From, i64)) -> RelationAggregateDef<From, i64> {
        RelationAggregateDef::new(Arc::new(ManyToManyAggregate {
            relation: self,
            kind: AggregateKind::CountAll,
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn count_distinct<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, i64),
    ) -> RelationAggregateDef<From, i64> {
        RelationAggregateDef::new(Arc::new(ManyToManyAggregate {
            relation: self,
            kind: AggregateKind::CountDistinct(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn sum<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, Option<Value>),
    ) -> RelationAggregateDef<From, Option<Value>>
    where
        Value: ToDbValue + FromDbValue + Send + Sync + 'static,
    {
        RelationAggregateDef::new(Arc::new(ManyToManyAggregate {
            relation: self,
            kind: AggregateKind::Sum(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn avg<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, Option<Value>),
    ) -> RelationAggregateDef<From, Option<Value>>
    where
        Value: ToDbValue + FromDbValue + Send + Sync + 'static,
    {
        RelationAggregateDef::new(Arc::new(ManyToManyAggregate {
            relation: self,
            kind: AggregateKind::Avg(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn min<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, Option<Value>),
    ) -> RelationAggregateDef<From, Option<Value>>
    where
        Value: ToDbValue + FromDbValue + Send + Sync + 'static,
    {
        RelationAggregateDef::new(Arc::new(ManyToManyAggregate {
            relation: self,
            kind: AggregateKind::Min(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn max<Value>(
        self,
        column: Column<To, Value>,
        attach: fn(&mut From, Option<Value>),
    ) -> RelationAggregateDef<From, Option<Value>>
    where
        Value: ToDbValue + FromDbValue + Send + Sync + 'static,
    {
        RelationAggregateDef::new(Arc::new(ManyToManyAggregate {
            relation: self,
            kind: AggregateKind::Max(column.column_ref()),
            attach: Arc::new(attach),
            _marker: PhantomData,
        }))
    }

    pub fn node(&self) -> RelationNode {
        RelationNode {
            name: self.name.clone(),
            kind: RelationKind::ManyToMany,
            target: self.target_table.table_ref(),
            local_key: self.parent_column.clone(),
            foreign_key: self.target_column.clone(),
            pivot: Some(super::ast::PivotNode {
                table: self.pivot_table.clone(),
                local_key: self.pivot_parent_column.clone(),
                foreign_key: self.pivot_target_column.clone(),
            }),
            filters: self.filter.clone(),
            children: self
                .children
                .iter()
                .map(|child| child.node())
                .chain(
                    self.child_aggregates
                        .iter()
                        .map(|aggregate| aggregate.node()),
                )
                .collect(),
            aggregates: Vec::new(),
        }
    }

    pub(crate) fn scoped_with_filter(mut self, filter: Option<Condition>) -> Self {
        if let Some(filter) = filter {
            self.filter = merge_condition(self.filter.take(), filter);
        }
        self
    }

    pub(crate) fn exists_condition(&self) -> Condition {
        let mut exists_select = SelectNode::from(self.target_table.table_ref());
        exists_select.columns = vec![SelectItem::new(Expr::raw("1"))];
        exists_select.joins.push(JoinNode {
            kind: JoinKind::Inner,
            table: self.pivot_table.clone().into(),
            lateral: false,
            on: Some(Condition::compare(
                Expr::column(self.target_column.clone()),
                ComparisonOp::Eq,
                Expr::column(self.pivot_target_column.clone()),
            )),
        });
        let condition = Condition::compare(
            Expr::column(self.pivot_parent_column.clone()),
            ComparisonOp::Eq,
            Expr::column(self.parent_column.clone()),
        );
        exists_select.condition = Some(match self.filter.clone() {
            Some(filter) => Condition::and([filter, condition]),
            None => condition,
        });
        Condition::exists(QueryAst::select(exists_select))
    }
}

#[async_trait]
impl<From, To> RelationLoader<From> for RelationDef<From, To>
where
    From: Model,
    To: Model,
{
    fn node(&self) -> RelationNode {
        self.node()
    }

    async fn load(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()> {
        let keys = collect_relation_keys(parents, &self.parent_key);
        let child_entries = if keys.is_empty() {
            Vec::new()
        } else {
            let mut query = ModelQuery::new(self.target_table).where_(Condition::InList {
                expr: Expr::column(self.target_column.clone()),
                values: keys,
            });
            if let Some(filter) = self.filter.clone() {
                query = query.where_(filter);
            }
            for extension in &self.child_extensions {
                query = query.with_extension_boxed(extension.clone());
            }
            for child in &self.children {
                query = query.with_boxed(child.clone());
            }
            for aggregate in &self.child_aggregates {
                query = query.with_aggregate_boxed(aggregate.clone());
            }
            query.fetch_entries_dyn(executor).await?
        };

        let mut grouped: HashMap<String, Vec<To>> = HashMap::new();
        for (record, model) in child_entries {
            let key = record
                .get(&self.target_column.name)
                .ok_or_else(|| {
                    Error::message(format!(
                        "missing target relation key `{}` in eager-loaded record",
                        self.target_column.name
                    ))
                })?
                .relation_key();
            grouped.entry(key).or_default().push(model);
        }

        for parent in parents.iter_mut() {
            let values = (self.parent_key)(parent)
                .map(|key| {
                    grouped
                        .get(&key.relation_key())
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            match &self.attach {
                RelationAttach::Many(attach) => attach(parent, values),
                RelationAttach::One(attach) => attach(parent, values.into_iter().next()),
            }
        }

        Ok(())
    }

    async fn load_missing(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()> {
        let is_loaded_fn = match &self.is_loaded {
            Some(f) => f,
            None => return self.load(executor, parents).await,
        };

        // Find indices of parents that need loading
        let unloaded_indices: Vec<usize> = parents
            .iter()
            .enumerate()
            .filter(|(_, p)| !is_loaded_fn(p))
            .map(|(i, _)| i)
            .collect();

        if unloaded_indices.is_empty() {
            return Ok(());
        }

        // Collect keys only from unloaded parents (with dedup)
        let mut keys = Vec::new();
        let mut seen = BTreeSet::new();
        for &i in &unloaded_indices {
            if let Some(key) = (self.parent_key)(&parents[i]) {
                if seen.insert(key.relation_key()) {
                    keys.push(key);
                }
            }
        }

        // Query (same logic as load)
        let child_entries = if keys.is_empty() {
            Vec::new()
        } else {
            let mut query = ModelQuery::new(self.target_table).where_(Condition::InList {
                expr: Expr::column(self.target_column.clone()),
                values: keys,
            });
            if let Some(filter) = self.filter.clone() {
                query = query.where_(filter);
            }
            for extension in &self.child_extensions {
                query = query.with_extension_boxed(extension.clone());
            }
            for child in &self.children {
                query = query.with_boxed(child.clone());
            }
            for aggregate in &self.child_aggregates {
                query = query.with_aggregate_boxed(aggregate.clone());
            }
            query.fetch_entries_dyn(executor).await?
        };

        // Group (same logic as load)
        let mut grouped: HashMap<String, Vec<To>> = HashMap::new();
        for (record, model) in child_entries {
            let key = record
                .get(&self.target_column.name)
                .ok_or_else(|| {
                    Error::message(format!(
                        "missing target relation key `{}` in eager-loaded record",
                        self.target_column.name
                    ))
                })?
                .relation_key();
            grouped.entry(key).or_default().push(model);
        }

        // Attach only to unloaded parents
        for &i in &unloaded_indices {
            let parent = &mut parents[i];
            let values = (self.parent_key)(parent)
                .map(|key| {
                    grouped
                        .get(&key.relation_key())
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            match &self.attach {
                RelationAttach::Many(attach) => attach(parent, values),
                RelationAttach::One(attach) => attach(parent, values.into_iter().next()),
            }
        }

        Ok(())
    }
}

#[async_trait]
impl<From, To, Pivot> RelationLoader<From> for ManyToManyDef<From, To, Pivot>
where
    From: Model,
    To: Model,
    Pivot: Clone + Send + Sync + 'static,
{
    fn node(&self) -> RelationNode {
        self.node()
    }

    async fn load(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()> {
        let keys = collect_relation_keys(parents, &self.parent_key);
        if keys.is_empty() {
            for parent in parents.iter_mut() {
                (self.attach)(parent, Vec::new());
            }
            return Ok(());
        }

        let mut select = SelectNode::from(self.target_table.table_ref());
        select.columns = self.target_table.all_select_items();
        select.columns.push(
            SelectItem::new(Expr::column(self.pivot_parent_column.clone()))
                .aliased(RELATION_GROUP_KEY_ALIAS),
        );
        if let Some(pivot_attacher) = &self.pivot_attacher {
            select
                .columns
                .extend(pivot_attacher.select_items(&self.pivot_table.name)?);
        }
        select.joins.push(JoinNode {
            kind: JoinKind::Inner,
            table: self.pivot_table.clone().into(),
            lateral: false,
            on: Some(Condition::compare(
                Expr::column(self.target_column.clone()),
                ComparisonOp::Eq,
                Expr::column(self.pivot_target_column.clone()),
            )),
        });
        let condition = Condition::InList {
            expr: Expr::column(self.pivot_parent_column.clone()),
            values: keys,
        };
        select.condition = Some(match self.filter.clone() {
            Some(filter) => Condition::and([filter, condition]),
            None => condition,
        });

        let compiled = PostgresCompiler::compile(&QueryAst::select(select))?;
        let records = executor.query_records(&compiled).await?;
        let mut models = records
            .iter()
            .map(|record| self.target_table.hydrate_record(record))
            .collect::<Result<Vec<_>>>()?;

        if let Some(pivot_attacher) = &self.pivot_attacher {
            for (record, model) in records.iter().zip(models.iter_mut()) {
                pivot_attacher.attach(record, model)?;
            }
        }

        register_model_records(self.target_table, &records);

        for extension in &self.child_extensions {
            extension.load(executor, &models).await?;
        }

        for child in &self.children {
            child.load(executor, &mut models).await?;
        }

        for aggregate in &self.child_aggregates {
            aggregate.load(executor, &mut models).await?;
        }

        let mut grouped: HashMap<String, Vec<To>> = HashMap::new();
        for (record, model) in records.into_iter().zip(models.into_iter()) {
            let key = record
                .get(RELATION_GROUP_KEY_ALIAS)
                .ok_or_else(|| Error::message("missing many-to-many group key in record"))?
                .relation_key();
            grouped.entry(key).or_default().push(model);
        }

        for parent in parents.iter_mut() {
            let children = (self.parent_key)(parent)
                .map(|key| {
                    grouped
                        .get(&key.relation_key())
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            (self.attach)(parent, children);
        }

        Ok(())
    }

    async fn load_missing(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()> {
        let is_loaded_fn = match &self.is_loaded {
            Some(f) => f,
            None => return self.load(executor, parents).await,
        };

        let unloaded_indices: Vec<usize> = parents
            .iter()
            .enumerate()
            .filter(|(_, p)| !is_loaded_fn(p))
            .map(|(i, _)| i)
            .collect();

        if unloaded_indices.is_empty() {
            return Ok(());
        }

        // Collect keys only from unloaded parents
        let mut keys = Vec::new();
        let mut seen = BTreeSet::new();
        for &i in &unloaded_indices {
            if let Some(key) = (self.parent_key)(&parents[i]) {
                if seen.insert(key.relation_key()) {
                    keys.push(key);
                }
            }
        }

        if keys.is_empty() {
            for &i in &unloaded_indices {
                let parent = &mut parents[i];
                (self.attach)(parent, Vec::new());
            }
            return Ok(());
        }

        // Same query logic as load
        let mut select = SelectNode::from(self.target_table.table_ref());
        select.columns = self.target_table.all_select_items();
        select.columns.push(
            SelectItem::new(Expr::column(self.pivot_parent_column.clone()))
                .aliased(RELATION_GROUP_KEY_ALIAS),
        );
        if let Some(pivot_attacher) = &self.pivot_attacher {
            select
                .columns
                .extend(pivot_attacher.select_items(&self.pivot_table.name)?);
        }
        select.joins.push(JoinNode {
            kind: JoinKind::Inner,
            table: self.pivot_table.clone().into(),
            lateral: false,
            on: Some(Condition::compare(
                Expr::column(self.target_column.clone()),
                ComparisonOp::Eq,
                Expr::column(self.pivot_target_column.clone()),
            )),
        });
        let condition = Condition::InList {
            expr: Expr::column(self.pivot_parent_column.clone()),
            values: keys,
        };
        select.condition = Some(match self.filter.clone() {
            Some(filter) => Condition::and([filter, condition]),
            None => condition,
        });

        let compiled = PostgresCompiler::compile(&QueryAst::select(select))?;
        let records = executor.query_records(&compiled).await?;
        let mut models = records
            .iter()
            .map(|record| self.target_table.hydrate_record(record))
            .collect::<Result<Vec<_>>>()?;

        if let Some(pivot_attacher) = &self.pivot_attacher {
            for (record, model) in records.iter().zip(models.iter_mut()) {
                pivot_attacher.attach(record, model)?;
            }
        }

        register_model_records(self.target_table, &records);

        for extension in &self.child_extensions {
            extension.load(executor, &models).await?;
        }

        for child in &self.children {
            child.load(executor, &mut models).await?;
        }
        for aggregate in &self.child_aggregates {
            aggregate.load(executor, &mut models).await?;
        }

        let mut grouped: HashMap<String, Vec<To>> = HashMap::new();
        for (record, model) in records.into_iter().zip(models.into_iter()) {
            let key = record
                .get(RELATION_GROUP_KEY_ALIAS)
                .ok_or_else(|| Error::message("missing many-to-many group key in record"))?
                .relation_key();
            grouped.entry(key).or_default().push(model);
        }

        // Attach only to unloaded parents
        for &i in &unloaded_indices {
            let parent = &mut parents[i];
            let children = (self.parent_key)(parent)
                .map(|key| {
                    grouped
                        .get(&key.relation_key())
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            (self.attach)(parent, children);
        }

        Ok(())
    }
}

#[derive(Clone)]
struct ScalarRelationAggregate<From, To: 'static, Value: 'static> {
    relation: RelationDef<From, To>,
    kind: AggregateKind,
    attach: Arc<AttachAggregateFn<From, Value>>,
    _marker: PhantomData<fn() -> Value>,
}

#[async_trait]
impl<From, To> RelationAggregateLoader<From> for ScalarRelationAggregate<From, To, i64>
where
    From: Model,
    To: Model,
{
    fn node(&self) -> RelationNode {
        let mut node = self.relation.node();
        node.aggregates.push(self.kind.clone().node());
        node
    }

    async fn load(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()> {
        let keys = collect_relation_keys(parents, &self.relation.parent_key);
        let grouped =
            execute_relation_aggregate_query(executor, &self.relation, self.kind.clone(), keys)
                .await?;
        for parent in parents.iter_mut() {
            let value = (self.relation.parent_key)(parent)
                .and_then(|key| grouped.get(&key.relation_key()))
                .and_then(|record| record.decode::<i64>(RELATION_AGGREGATE_ALIAS).ok())
                .unwrap_or(0);
            (self.attach)(parent, value);
        }
        Ok(())
    }
}

#[async_trait]
impl<From, To, Value> RelationAggregateLoader<From>
    for ScalarRelationAggregate<From, To, Option<Value>>
where
    From: Model,
    To: Model,
    Value: ToDbValue + FromDbValue + Send + Sync + 'static,
{
    fn node(&self) -> RelationNode {
        let mut node = self.relation.node();
        node.aggregates.push(self.kind.clone().node());
        node
    }

    async fn load(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()> {
        let keys = collect_relation_keys(parents, &self.relation.parent_key);
        let grouped =
            execute_relation_aggregate_query(executor, &self.relation, self.kind.clone(), keys)
                .await?;
        for parent in parents.iter_mut() {
            let value = (self.relation.parent_key)(parent)
                .and_then(|key| grouped.get(&key.relation_key()))
                .map(|record| record.decode::<Option<Value>>(RELATION_AGGREGATE_ALIAS))
                .transpose()?
                .flatten();
            (self.attach)(parent, value);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct ManyToManyAggregate<From, To: 'static, Pivot: 'static, Value: 'static> {
    relation: ManyToManyDef<From, To, Pivot>,
    kind: AggregateKind,
    attach: Arc<AttachAggregateFn<From, Value>>,
    _marker: PhantomData<fn() -> Value>,
}

#[async_trait]
impl<From, To, Pivot> RelationAggregateLoader<From> for ManyToManyAggregate<From, To, Pivot, i64>
where
    From: Model,
    To: Model,
    Pivot: Clone + Send + Sync + 'static,
{
    fn node(&self) -> RelationNode {
        let mut node = self.relation.node();
        node.aggregates.push(self.kind.clone().node());
        node
    }

    async fn load(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()> {
        let keys = collect_relation_keys(parents, &self.relation.parent_key);
        let grouped =
            execute_many_to_many_aggregate_query(executor, &self.relation, self.kind.clone(), keys)
                .await?;
        for parent in parents.iter_mut() {
            let value = (self.relation.parent_key)(parent)
                .and_then(|key| grouped.get(&key.relation_key()))
                .and_then(|record| record.decode::<i64>(RELATION_AGGREGATE_ALIAS).ok())
                .unwrap_or(0);
            (self.attach)(parent, value);
        }
        Ok(())
    }
}

#[async_trait]
impl<From, To, Pivot, Value> RelationAggregateLoader<From>
    for ManyToManyAggregate<From, To, Pivot, Option<Value>>
where
    From: Model,
    To: Model,
    Pivot: Clone + Send + Sync + 'static,
    Value: ToDbValue + FromDbValue + Send + Sync + 'static,
{
    fn node(&self) -> RelationNode {
        let mut node = self.relation.node();
        node.aggregates.push(self.kind.clone().node());
        node
    }

    async fn load(&self, executor: &dyn QueryExecutor, parents: &mut [From]) -> Result<()> {
        let keys = collect_relation_keys(parents, &self.relation.parent_key);
        let grouped =
            execute_many_to_many_aggregate_query(executor, &self.relation, self.kind.clone(), keys)
                .await?;
        for parent in parents.iter_mut() {
            let value = (self.relation.parent_key)(parent)
                .and_then(|key| grouped.get(&key.relation_key()))
                .map(|record| record.decode::<Option<Value>>(RELATION_AGGREGATE_ALIAS))
                .transpose()?
                .flatten();
            (self.attach)(parent, value);
        }
        Ok(())
    }
}

pub fn has_many<From, To, Key>(
    local_key: Column<From, Key>,
    foreign_key: Column<To, Key>,
    parent_key: fn(&From) -> Key,
    attach: fn(&mut From, Vec<To>),
) -> RelationDef<From, To>
where
    From: Model,
    To: Model,
    Key: ToDbValue + 'static,
{
    RelationDef {
        name: infer_collection_relation_name(To::table_meta().name()),
        kind: RelationKind::HasMany,
        parent_column: local_key.column_ref(),
        target_column: foreign_key.column_ref(),
        target_table: To::table_meta(),
        parent_key: Arc::new(move |parent| Some(parent_key(parent).to_db_value())),
        attach: RelationAttach::Many(Arc::new(attach)),
        is_loaded: None,
        filter: None,
        children: Vec::new(),
        child_extensions: Vec::new(),
        child_aggregates: Vec::new(),
    }
}

pub fn has_one<From, To, Key>(
    local_key: Column<From, Key>,
    foreign_key: Column<To, Key>,
    parent_key: fn(&From) -> Key,
    attach: fn(&mut From, Option<To>),
) -> RelationDef<From, To>
where
    From: Model,
    To: Model,
    Key: ToDbValue + 'static,
{
    RelationDef {
        name: infer_singular_relation_name(To::table_meta().name()),
        kind: RelationKind::HasOne,
        parent_column: local_key.column_ref(),
        target_column: foreign_key.column_ref(),
        target_table: To::table_meta(),
        parent_key: Arc::new(move |parent| Some(parent_key(parent).to_db_value())),
        attach: RelationAttach::One(Arc::new(attach)),
        is_loaded: None,
        filter: None,
        children: Vec::new(),
        child_extensions: Vec::new(),
        child_aggregates: Vec::new(),
    }
}

pub fn belongs_to<From, To, Key>(
    foreign_key: Column<From, Key>,
    owner_key: Column<To, Key>,
    parent_key: fn(&From) -> Option<Key>,
    attach: fn(&mut From, Option<To>),
) -> RelationDef<From, To>
where
    From: Model,
    To: Model,
    Key: ToDbValue + 'static,
{
    RelationDef {
        name: infer_singular_relation_name(To::table_meta().name()),
        kind: RelationKind::BelongsTo,
        parent_column: foreign_key.column_ref(),
        target_column: owner_key.column_ref(),
        target_table: To::table_meta(),
        parent_key: Arc::new(move |parent| parent_key(parent).map(ToDbValue::to_db_value)),
        attach: RelationAttach::One(Arc::new(attach)),
        is_loaded: None,
        filter: None,
        children: Vec::new(),
        child_extensions: Vec::new(),
        child_aggregates: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn many_to_many<From, To, Pivot, LocalKey, TargetKey>(
    local_key: Column<From, LocalKey>,
    pivot_table: &'static str,
    pivot_local_key: &'static str,
    pivot_related_key: &'static str,
    target_key: Column<To, TargetKey>,
    parent_key: fn(&From) -> LocalKey,
    attach: fn(&mut From, Vec<To>),
) -> ManyToManyDef<From, To, Pivot>
where
    From: Model,
    To: Model,
    LocalKey: ToDbValue + 'static,
    TargetKey: 'static,
    Pivot: Clone + Send + Sync + 'static,
{
    ManyToManyDef {
        name: infer_collection_relation_name(To::table_meta().name()),
        parent_column: local_key.column_ref(),
        pivot_table: TableRef::new(pivot_table),
        pivot_parent_column: ColumnRef::new(pivot_table, pivot_local_key)
            .typed(local_key.db_type()),
        pivot_target_column: ColumnRef::new(pivot_table, pivot_related_key)
            .typed(target_key.db_type()),
        target_column: target_key.column_ref(),
        target_table: To::table_meta(),
        parent_key: Arc::new(move |parent| Some(parent_key(parent).to_db_value())),
        attach: Arc::new(attach),
        is_loaded: None,
        filter: None,
        children: Vec::new(),
        child_extensions: Vec::new(),
        child_aggregates: Vec::new(),
        pivot_attacher: None,
        _pivot: PhantomData,
    }
}

fn infer_collection_relation_name(table_name: &str) -> String {
    relation_basename(table_name).to_string()
}

fn infer_singular_relation_name(table_name: &str) -> String {
    singularize_relation_name(relation_basename(table_name))
}

fn relation_basename(table_name: &str) -> &str {
    table_name.rsplit('.').next().unwrap_or(table_name)
}

fn singularize_relation_name(name: &str) -> String {
    if let Some(stem) = name.strip_suffix("ies") {
        return format!("{stem}y");
    }

    for suffix in ["sses", "shes", "ches", "xes", "zes"] {
        if let Some(stem) = name.strip_suffix(suffix) {
            return format!("{stem}{}", &suffix[..suffix.len() - 2]);
        }
    }

    if let Some(stem) = name.strip_suffix('s') {
        if !stem.ends_with('s') {
            return stem.to_string();
        }
    }

    name.to_string()
}

fn collect_relation_keys<From>(
    parents: &[From],
    parent_key: &Arc<ParentKeyFn<From>>,
) -> Vec<DbValue> {
    let mut keys = Vec::new();
    let mut seen = BTreeSet::new();

    for parent in parents {
        if let Some(key) = parent_key(parent) {
            if seen.insert(key.relation_key()) {
                keys.push(key);
            }
        }
    }

    keys
}

async fn execute_relation_aggregate_query<From, To>(
    executor: &dyn QueryExecutor,
    relation: &RelationDef<From, To>,
    kind: AggregateKind,
    keys: Vec<DbValue>,
) -> Result<HashMap<String, DbRecord>>
where
    From: Model,
    To: Model,
{
    execute_grouped_aggregate_query(
        executor,
        relation.target_table.table_ref(),
        relation.filter.clone(),
        relation.target_column.clone(),
        None,
        keys,
        kind,
    )
    .await
}

async fn execute_many_to_many_aggregate_query<From, To, Pivot>(
    executor: &dyn QueryExecutor,
    relation: &ManyToManyDef<From, To, Pivot>,
    kind: AggregateKind,
    keys: Vec<DbValue>,
) -> Result<HashMap<String, DbRecord>>
where
    From: Model,
    To: Model,
    Pivot: Clone + Send + Sync + 'static,
{
    execute_grouped_aggregate_query(
        executor,
        relation.target_table.table_ref(),
        relation.filter.clone(),
        relation.pivot_parent_column.clone(),
        Some(JoinNode {
            kind: JoinKind::Inner,
            table: relation.pivot_table.clone().into(),
            lateral: false,
            on: Some(Condition::compare(
                Expr::column(relation.target_column.clone()),
                ComparisonOp::Eq,
                Expr::column(relation.pivot_target_column.clone()),
            )),
        }),
        keys,
        kind,
    )
    .await
}

async fn execute_grouped_aggregate_query(
    executor: &dyn QueryExecutor,
    from: TableRef,
    filter: Option<Condition>,
    group_key_column: ColumnRef,
    join: Option<JoinNode>,
    keys: Vec<DbValue>,
    kind: AggregateKind,
) -> Result<HashMap<String, DbRecord>> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let mut select = SelectNode::from(from);
    select.columns =
        vec![SelectItem::new(Expr::column(group_key_column.clone()))
            .aliased(RELATION_GROUP_KEY_ALIAS)];
    if let Some(join) = join {
        select.joins.push(join);
    }
    let condition = Condition::InList {
        expr: Expr::column(group_key_column.clone()),
        values: keys,
    };
    select.condition = Some(match filter {
        Some(filter) => Condition::and([filter, condition]),
        None => condition,
    });
    select.group_by.push(Expr::column(group_key_column));
    select.aggregates.push(kind.node());

    let compiled = PostgresCompiler::compile(&QueryAst::select(select))?;
    let rows = executor.query_records(&compiled).await?;
    let mut grouped = HashMap::new();
    for row in rows {
        let key = row
            .get(RELATION_GROUP_KEY_ALIAS)
            .ok_or_else(|| Error::message("missing relation aggregate group key"))?
            .relation_key();
        grouped.insert(key, row);
    }
    Ok(grouped)
}

fn merge_condition(existing: Option<Condition>, next: Condition) -> Option<Condition> {
    Some(match existing {
        Some(existing) => Condition::and([existing, next]),
        None => next,
    })
}

#[cfg(test)]
mod tests {
    use super::{infer_collection_relation_name, infer_singular_relation_name};

    #[test]
    fn infers_collection_relation_names_from_table_names() {
        assert_eq!(infer_collection_relation_name("merchants"), "merchants");
        assert_eq!(
            infer_collection_relation_name("public.order_items"),
            "order_items"
        );
    }

    #[test]
    fn infers_singular_relation_names_from_plural_table_names() {
        assert_eq!(infer_singular_relation_name("countries"), "country");
        assert_eq!(
            infer_singular_relation_name("public.categories"),
            "category"
        );
        assert_eq!(infer_singular_relation_name("products"), "product");
    }
}
