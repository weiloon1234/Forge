use std::{collections::VecDeque, future::Future, marker::PhantomData, pin::Pin};

use crate::foundation::{AppContext, Error, Result};
use futures_util::stream::{self, BoxStream, StreamExt};

use super::aggregate::{
    count_query_ast, execute_scalar_projection_on_ast, wrap_query_for_alias_aggregate,
    AggregateProjection,
};
use super::ast::{
    BinaryOperator, CaseExpr, CaseWhen, ColumnRef, ComparisonOp, Condition, CteNode, Expr,
    FromItem, InsertNode, InsertSource, JoinKind, JoinNode, JsonPathExpr, JsonPathMode,
    JsonPathSegment, JsonPredicateOp, JsonPredicateValue, LockBehavior, LockClause, LockStrength,
    OnConflictAction, OnConflictNode, OnConflictTarget, OnConflictUpdate, OrderBy, QueryAst,
    QueryBody, SelectItem, SelectNode, SetOperationNode, SetOperator, UpdateNode, WindowFrame,
    WindowFrameBound, WindowFrameUnits, WindowSpec,
};
use super::compiler::PostgresCompiler;
use super::model::{
    Column, CreateDraft, FromDbValue, IntoFieldValue, Model, ModelCreatedEvent, ModelCreatingEvent,
    ModelDeletedEvent, ModelDeletingEvent, ModelHookContext, ModelLifecycle,
    ModelLifecycleSnapshot, ModelUpdatedEvent, ModelUpdatingEvent, ModelWriteExecutor, TableMeta,
    UpdateDraft,
};
use super::projection::{Projection, ProjectionField, ProjectionMeta};
use super::relation::{
    AnyRelation, AnyRelationAggregate, ManyToManyDef, RelationAggregateDef, RelationDef,
};
use super::runtime::{DbRecord, DbRecordStream, QueryExecutionOptions, QueryExecutor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pagination {
    pub page: u64,
    pub per_page: u64,
}

impl Pagination {
    pub fn new(page: u64, per_page: u64) -> Self {
        Self {
            page: page.max(1),
            per_page: per_page.max(1),
        }
    }

    pub fn offset(&self) -> u64 {
        (self.page - 1) * self.per_page
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paginated<T> {
    pub data: Vec<T>,
    pub pagination: Pagination,
    pub total: u64,
}

pub struct Case;
pub struct Sql;
pub struct Window;

#[derive(Clone, Debug, Default)]
pub struct CaseBuilder {
    expr: CaseExpr,
}

impl Case {
    pub fn when(condition: Condition, result: impl Into<Expr>) -> CaseBuilder {
        CaseBuilder::default().when(condition, result)
    }
}

impl Sql {
    pub fn function(name: impl Into<String>, args: impl IntoIterator<Item = Expr>) -> Expr {
        Expr::function(name, args)
    }

    pub fn coalesce(args: impl IntoIterator<Item = Expr>) -> Expr {
        Expr::function("COALESCE", args)
    }

    pub fn lower(expr: impl Into<Expr>) -> Expr {
        Expr::function("LOWER", [expr.into()])
    }

    pub fn upper(expr: impl Into<Expr>) -> Expr {
        Expr::function("UPPER", [expr.into()])
    }

    pub fn date_trunc(granularity: impl Into<String>, expr: impl Into<Expr>) -> Expr {
        Expr::function("DATE_TRUNC", [Expr::value(granularity.into()), expr.into()])
    }

    pub fn extract(field: impl Into<String>, expr: impl Into<Expr>) -> Expr {
        Expr::function("EXTRACT", [Expr::value(field.into()), expr.into()])
    }

    pub fn now() -> Expr {
        Expr::function("NOW", std::iter::empty())
    }

    pub fn not(expr: impl Into<Expr>) -> Expr {
        Expr::unary(super::ast::UnaryOperator::Not, expr)
    }

    pub fn negate(expr: impl Into<Expr>) -> Expr {
        Expr::unary(super::ast::UnaryOperator::Negate, expr)
    }

    pub fn add(left: impl Into<Expr>, right: impl Into<Expr>) -> Expr {
        Expr::binary(left, BinaryOperator::Add, right)
    }

    pub fn subtract(left: impl Into<Expr>, right: impl Into<Expr>) -> Expr {
        Expr::binary(left, BinaryOperator::Subtract, right)
    }

    pub fn multiply(left: impl Into<Expr>, right: impl Into<Expr>) -> Expr {
        Expr::binary(left, BinaryOperator::Multiply, right)
    }

    pub fn divide(left: impl Into<Expr>, right: impl Into<Expr>) -> Expr {
        Expr::binary(left, BinaryOperator::Divide, right)
    }

    pub fn concat(left: impl Into<Expr>, right: impl Into<Expr>) -> Expr {
        Expr::binary(left, BinaryOperator::Concat, right)
    }

    pub fn op(left: impl Into<Expr>, operator: impl Into<String>, right: impl Into<Expr>) -> Expr {
        Expr::binary(left, BinaryOperator::Custom(operator.into()), right)
    }
}

#[derive(Clone, Debug, Default)]
pub struct WindowBuilder {
    spec: WindowSpec,
}

impl Window {
    pub fn partition_by(expr: impl Into<Expr>) -> WindowBuilder {
        WindowBuilder::default().partition_by(expr)
    }

    pub fn order_by(order: OrderBy) -> WindowBuilder {
        WindowBuilder::default().order_by(order)
    }

    pub fn over(function: impl Into<Expr>, builder: WindowBuilder) -> Expr {
        Expr::window(function, builder.finish())
    }
}

impl WindowBuilder {
    pub fn partition_by(mut self, expr: impl Into<Expr>) -> Self {
        self.spec.partition_by.push(expr.into());
        self
    }

    pub fn order_by(mut self, order: OrderBy) -> Self {
        self.spec.order_by.push(order);
        self
    }

    pub fn rows_between(mut self, start: WindowFrameBound, end: WindowFrameBound) -> Self {
        self.spec.frame = Some(WindowFrame {
            units: WindowFrameUnits::Rows,
            start,
            end: Some(end),
        });
        self
    }

    pub fn range_between(mut self, start: WindowFrameBound, end: WindowFrameBound) -> Self {
        self.spec.frame = Some(WindowFrame {
            units: WindowFrameUnits::Range,
            start,
            end: Some(end),
        });
        self
    }

    pub fn finish(self) -> WindowSpec {
        self.spec
    }
}

impl CaseBuilder {
    pub fn when(mut self, condition: Condition, result: impl Into<Expr>) -> Self {
        self.expr.whens.push(CaseWhen {
            condition,
            result: Box::new(result.into()),
        });
        self
    }

    pub fn else_(mut self, result: impl Into<Expr>) -> Expr {
        self.expr.else_expr = Some(Box::new(result.into()));
        Expr::from(self.expr)
    }

    pub fn end(self) -> Expr {
        Expr::from(self.expr)
    }
}

#[derive(Clone, Debug)]
pub struct JsonExprBuilder {
    expr: Expr,
    path: Vec<JsonPathSegment>,
}

impl JsonExprBuilder {
    pub fn new(expr: impl Into<Expr>) -> Self {
        Self {
            expr: expr.into(),
            path: Vec::new(),
        }
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.path.push(JsonPathSegment::Key(key.into()));
        self
    }

    pub fn index(mut self, index: i64) -> Self {
        self.path.push(JsonPathSegment::Index(index));
        self
    }

    pub fn as_json(self) -> Expr {
        Expr::from(JsonPathExpr {
            expr: Box::new(self.expr),
            path: self.path,
            mode: JsonPathMode::Json,
        })
    }

    pub fn as_text(self) -> Expr {
        Expr::from(JsonPathExpr {
            expr: Box::new(self.expr),
            path: self.path,
            mode: JsonPathMode::Text,
        })
    }

    pub fn contains(self, value: impl Into<serde_json::Value>) -> Condition {
        Condition::json(
            self.as_json(),
            JsonPredicateOp::Contains,
            JsonPredicateValue::Json(value.into()),
        )
    }

    pub fn contained_by(self, value: impl Into<serde_json::Value>) -> Condition {
        Condition::json(
            self.as_json(),
            JsonPredicateOp::ContainedBy,
            JsonPredicateValue::Json(value.into()),
        )
    }

    pub fn has_key(self, key: impl Into<String>) -> Condition {
        Condition::json(
            self.as_json(),
            JsonPredicateOp::HasKey,
            JsonPredicateValue::Key(key.into()),
        )
    }

    pub fn has_any_keys<I, S>(self, keys: I) -> Condition
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Condition::json(
            self.as_json(),
            JsonPredicateOp::HasAnyKeys,
            JsonPredicateValue::Keys(keys.into_iter().map(Into::into).collect()),
        )
    }

    pub fn has_all_keys<I, S>(self, keys: I) -> Condition
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Condition::json(
            self.as_json(),
            JsonPredicateOp::HasAllKeys,
            JsonPredicateValue::Keys(keys.into_iter().map(Into::into).collect()),
        )
    }
}

impl Expr {
    pub fn json(self) -> JsonExprBuilder {
        JsonExprBuilder::new(self)
    }
}

#[derive(Clone, Debug)]
pub struct Cte {
    node: CteNode,
}

impl Cte {
    pub fn new(name: impl Into<String>, query: impl Into<QueryAst>) -> Self {
        Self {
            node: CteNode {
                name: name.into(),
                query: Box::new(query.into()),
                recursive: false,
                materialization: None,
            },
        }
    }

    pub fn materialized(mut self) -> Self {
        self.node.materialization = Some(super::ast::CteMaterialization::Materialized);
        self
    }

    pub fn not_materialized(mut self) -> Self {
        self.node.materialization = Some(super::ast::CteMaterialization::NotMaterialized);
        self
    }

    pub fn recursive(mut self) -> Self {
        self.node.recursive = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    ast: QueryAst,
    options: QueryExecutionOptions,
}

impl From<Query> for QueryAst {
    fn from(value: Query) -> Self {
        value.ast
    }
}

impl Query {
    pub fn table(source: impl Into<FromItem>) -> Self {
        Self {
            ast: QueryAst::select(SelectNode::from(source)),
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn insert_into(table: impl Into<super::ast::TableRef>) -> Self {
        Self::insert_many_into(table)
    }

    pub fn insert_many_into(table: impl Into<super::ast::TableRef>) -> Self {
        Self {
            ast: QueryAst::insert(InsertNode {
                into: table.into(),
                source: InsertSource::Values(Vec::new()),
                on_conflict: None,
                returning: Vec::new(),
            }),
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn insert_select_into(
        table: impl Into<super::ast::TableRef>,
        select: impl Into<QueryAst>,
    ) -> Self {
        Self {
            ast: QueryAst::insert(InsertNode {
                into: table.into(),
                source: InsertSource::Select(Box::new(select.into())),
                on_conflict: None,
                returning: Vec::new(),
            }),
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn update_table(table: impl Into<super::ast::TableRef>) -> Self {
        Self {
            ast: QueryAst::update(UpdateNode {
                table: table.into(),
                values: Vec::new(),
                from: Vec::new(),
                condition: None,
                returning: Vec::new(),
            }),
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn delete_from(table: impl Into<super::ast::TableRef>) -> Self {
        Self {
            ast: QueryAst::delete(super::ast::DeleteNode {
                from: table.into(),
                using: Vec::new(),
                condition: None,
                returning: Vec::new(),
            }),
            options: QueryExecutionOptions::default(),
        }
    }

    fn from_ast(ast: QueryAst) -> Self {
        Self {
            ast,
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.options.label = Some(label.into());
        self
    }

    pub fn with_cte(mut self, cte: Cte) -> Self {
        self.ast.with.push(cte.node);
        self
    }

    pub fn distinct(mut self) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select.distinct = true;
        }
        self
    }

    pub fn select<I, C>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = C>,
        C: Into<super::ast::ColumnRef>,
    {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select.columns = columns
                .into_iter()
                .map(|column| SelectItem::new(Expr::column(column.into())))
                .collect();
        }
        self
    }

    pub fn select_item(mut self, item: SelectItem) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select.columns.push(item);
        }
        self
    }

    pub fn select_expr(mut self, expr: impl Into<Expr>, alias: impl Into<String>) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select
                .columns
                .push(SelectItem::new(expr).aliased(alias.into()));
        }
        self
    }

    pub fn select_aggregate<T>(mut self, projection: AggregateProjection<T>) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select.aggregates.push(projection.node());
        }
        self
    }

    pub fn join(mut self, kind: JoinKind, table: impl Into<FromItem>, on: Condition) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select.joins.push(JoinNode {
                kind,
                table: table.into(),
                lateral: false,
                on: Some(on),
            });
        }
        self
    }

    pub fn join_lateral(
        mut self,
        kind: JoinKind,
        table: impl Into<FromItem>,
        on: Option<Condition>,
    ) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select.joins.push(JoinNode {
                kind,
                table: table.into(),
                lateral: true,
                on,
            });
        }
        self
    }

    pub fn inner_join(self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.join(JoinKind::Inner, table, on)
    }

    pub fn left_join(self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.join(JoinKind::Left, table, on)
    }

    pub fn right_join(self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.join(JoinKind::Right, table, on)
    }

    pub fn full_outer_join(self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.join(JoinKind::Full, table, on)
    }

    pub fn cross_join(mut self, table: impl Into<FromItem>) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select.joins.push(JoinNode {
                kind: JoinKind::Cross,
                table: table.into(),
                lateral: false,
                on: None,
            });
        }
        self
    }

    pub fn left_join_lateral(self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.join_lateral(JoinKind::Left, table, Some(on))
    }

    pub fn cross_join_lateral(self, table: impl Into<FromItem>) -> Self {
        self.join_lateral(JoinKind::Cross, table, None)
    }

    pub fn inner_join_lateral(self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.join_lateral(JoinKind::Inner, table, Some(on))
    }

    pub fn where_(mut self, condition: Condition) -> Self {
        match &mut self.ast.body {
            QueryBody::Select(select) => {
                select.condition = merge_condition(select.condition.take(), condition);
            }
            QueryBody::Insert(insert) => {
                if let Some(OnConflictNode {
                    action: OnConflictAction::DoUpdate(conflict),
                    ..
                }) = &mut insert.on_conflict
                {
                    conflict.condition = merge_condition(conflict.condition.take(), condition);
                }
            }
            QueryBody::Update(update) => {
                update.condition = merge_condition(update.condition.take(), condition);
            }
            QueryBody::Delete(delete) => {
                delete.condition = merge_condition(delete.condition.take(), condition);
            }
            QueryBody::SetOperation(_) => {}
        }
        self
    }

    pub fn where_eq(
        self,
        column: impl Into<super::ast::ColumnRef>,
        value: impl Into<super::ast::DbValue>,
    ) -> Self {
        self.where_(Condition::compare(
            Expr::column(column.into()),
            ComparisonOp::Eq,
            Expr::value(value.into()),
        ))
    }

    pub fn group_by(mut self, expr: impl Into<Expr>) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select.group_by.push(expr.into());
        }
        self
    }

    pub fn having(mut self, condition: Condition) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            select.having = merge_condition(select.having.take(), condition);
        }
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        match &mut self.ast.body {
            QueryBody::Select(select) => select.limit = Some(limit),
            QueryBody::SetOperation(set) => set.limit = Some(limit),
            QueryBody::Insert(_) | QueryBody::Update(_) | QueryBody::Delete(_) => {}
        }
        self
    }

    pub fn offset(mut self, offset: u64) -> Self {
        match &mut self.ast.body {
            QueryBody::Select(select) => select.offset = Some(offset),
            QueryBody::SetOperation(set) => set.offset = Some(offset),
            QueryBody::Insert(_) | QueryBody::Update(_) | QueryBody::Delete(_) => {}
        }
        self
    }

    pub fn order_by(mut self, order: OrderBy) -> Self {
        match &mut self.ast.body {
            QueryBody::Select(select) => select.order_by.push(order),
            QueryBody::SetOperation(set) => set.order_by.push(order),
            QueryBody::Insert(_) | QueryBody::Update(_) | QueryBody::Delete(_) => {}
        }
        self
    }

    pub fn value(
        mut self,
        column: impl Into<super::ast::ColumnRef>,
        value: impl Into<super::ast::DbValue>,
    ) -> Self {
        match &mut self.ast.body {
            QueryBody::Insert(insert) => {
                push_insert_expr_value(insert, (column.into(), Expr::value(value.into())));
            }
            QueryBody::Update(update) => {
                update
                    .values
                    .push((column.into(), Expr::value(value.into())));
            }
            QueryBody::Select(_) | QueryBody::Delete(_) | QueryBody::SetOperation(_) => {}
        }
        self
    }

    pub fn values<I, C, V>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = (C, V)>,
        C: Into<super::ast::ColumnRef>,
        V: Into<super::ast::DbValue>,
    {
        for (column, value) in values {
            self = self.value(column, value);
        }
        self
    }

    pub fn row<I, C, V>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = (C, V)>,
        C: Into<super::ast::ColumnRef>,
        V: Into<super::ast::DbValue>,
    {
        if let QueryBody::Insert(insert) = &mut self.ast.body {
            push_insert_expr_row(
                insert,
                values
                    .into_iter()
                    .map(|(column, value)| (column.into(), Expr::value(value.into())))
                    .collect(),
            );
        }
        self
    }

    pub fn rows<R, I, C, V>(mut self, rows: R) -> Self
    where
        R: IntoIterator<Item = I>,
        I: IntoIterator<Item = (C, V)>,
        C: Into<super::ast::ColumnRef>,
        V: Into<super::ast::DbValue>,
    {
        for row in rows {
            self = self.row(row);
        }
        self
    }

    pub fn on_conflict_columns<I, C>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = C>,
        C: Into<super::ast::ColumnRef>,
    {
        if let QueryBody::Insert(insert) = &mut self.ast.body {
            insert.on_conflict = Some(OnConflictNode {
                target: Some(OnConflictTarget::Columns(
                    columns.into_iter().map(Into::into).collect(),
                )),
                action: current_conflict_action(insert.on_conflict.take()),
            });
        }
        self
    }

    pub fn on_conflict_constraint(mut self, constraint: impl Into<String>) -> Self {
        if let QueryBody::Insert(insert) = &mut self.ast.body {
            insert.on_conflict = Some(OnConflictNode {
                target: Some(OnConflictTarget::Constraint(constraint.into())),
                action: current_conflict_action(insert.on_conflict.take()),
            });
        }
        self
    }

    pub fn do_nothing(mut self) -> Self {
        if let QueryBody::Insert(insert) = &mut self.ast.body {
            upsert_node(insert).action = OnConflictAction::DoNothing;
        }
        self
    }

    pub fn do_update(mut self) -> Self {
        if let QueryBody::Insert(insert) = &mut self.ast.body {
            upsert_node(insert).action = OnConflictAction::DoUpdate(Box::new(OnConflictUpdate {
                assignments: Vec::new(),
                condition: None,
            }));
        }
        self
    }

    pub fn set(
        mut self,
        column: impl Into<super::ast::ColumnRef>,
        value: impl Into<super::ast::DbValue>,
    ) -> Self {
        self = self.set_expr(column, Expr::value(value.into()));
        self
    }

    pub fn set_expr(
        mut self,
        column: impl Into<super::ast::ColumnRef>,
        expr: impl Into<Expr>,
    ) -> Self {
        let column = column.into();
        let expr = expr.into();
        match &mut self.ast.body {
            QueryBody::Insert(insert) => {
                if let Some(OnConflictNode {
                    action: OnConflictAction::DoUpdate(conflict),
                    ..
                }) = &mut insert.on_conflict
                {
                    conflict.assignments.push((column, expr));
                }
            }
            QueryBody::Update(update) => update.values.push((column, expr)),
            QueryBody::Select(_) | QueryBody::Delete(_) | QueryBody::SetOperation(_) => {}
        }
        self
    }

    pub fn set_excluded(mut self, column: impl Into<super::ast::ColumnRef>) -> Self {
        let column = column.into();
        self = self.set_expr(column.clone(), Expr::excluded(column));
        self
    }

    pub fn from(mut self, source: impl Into<FromItem>) -> Self {
        if let QueryBody::Update(update) = &mut self.ast.body {
            update.from.push(source.into());
        }
        self
    }

    pub fn using(mut self, source: impl Into<FromItem>) -> Self {
        if let QueryBody::Delete(delete) = &mut self.ast.body {
            delete.using.push(source.into());
        }
        self
    }

    pub fn returning<I, C>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = C>,
        C: Into<super::ast::ColumnRef>,
    {
        let items = columns
            .into_iter()
            .map(|column| SelectItem::new(Expr::column(column.into())))
            .collect::<Vec<_>>();
        match &mut self.ast.body {
            QueryBody::Insert(insert) => insert.returning = items,
            QueryBody::Update(update) => update.returning = items,
            QueryBody::Delete(delete) => delete.returning = items,
            QueryBody::Select(_) | QueryBody::SetOperation(_) => {}
        }
        self
    }

    pub fn union(self, other: Self) -> Self {
        Self::from_ast(QueryAst::set_operation(SetOperationNode {
            left: Box::new(self.ast),
            operator: SetOperator::Union,
            right: Box::new(other.ast),
            order_by: Vec::new(),
            limit: None,
            offset: None,
        }))
    }

    pub fn union_all(self, other: Self) -> Self {
        Self::from_ast(QueryAst::set_operation(SetOperationNode {
            left: Box::new(self.ast),
            operator: SetOperator::UnionAll,
            right: Box::new(other.ast),
            order_by: Vec::new(),
            limit: None,
            offset: None,
        }))
    }

    pub fn ast(&self) -> &QueryAst {
        &self.ast
    }

    pub fn compile(&self) -> Result<super::compiler::CompiledSql> {
        PostgresCompiler::compile(&self.ast)
    }

    pub fn to_compiled_sql(&self) -> Result<super::compiler::CompiledSql> {
        self.compile()
    }

    pub fn for_update(mut self) -> Self {
        self = self.lock(LockStrength::Update);
        self
    }

    pub fn for_no_key_update(mut self) -> Self {
        self = self.lock(LockStrength::NoKeyUpdate);
        self
    }

    pub fn for_share(mut self) -> Self {
        self = self.lock(LockStrength::Share);
        self
    }

    pub fn for_key_share(mut self) -> Self {
        self = self.lock(LockStrength::KeyShare);
        self
    }

    pub fn of<I, S>(mut self, aliases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        if let QueryBody::Select(select) = &mut self.ast.body {
            let lock = select.lock.get_or_insert(LockClause {
                strength: LockStrength::Update,
                of: Vec::new(),
                behavior: LockBehavior::Wait,
            });
            lock.of.extend(aliases.into_iter().map(Into::into));
        }
        self
    }

    pub fn skip_locked(mut self) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            let lock = select.lock.get_or_insert(LockClause {
                strength: LockStrength::Update,
                of: Vec::new(),
                behavior: LockBehavior::Wait,
            });
            lock.behavior = LockBehavior::SkipLocked;
        }
        self
    }

    pub fn nowait(mut self) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            let lock = select.lock.get_or_insert(LockClause {
                strength: LockStrength::Update,
                of: Vec::new(),
                behavior: LockBehavior::Wait,
            });
            lock.behavior = LockBehavior::NoWait;
        }
        self
    }

    pub async fn get<E>(&self, executor: &E) -> Result<Vec<DbRecord>>
    where
        E: QueryExecutor,
    {
        let compiled = self.compile()?;
        executor
            .query_records_with(&compiled, self.options.clone())
            .await
    }

    pub async fn first<E>(&self, executor: &E) -> Result<Option<DbRecord>>
    where
        E: QueryExecutor,
    {
        let query = match &self.ast.body {
            QueryBody::Select(_) | QueryBody::SetOperation(_) => self.clone().limit(1),
            QueryBody::Insert(_) | QueryBody::Update(_) | QueryBody::Delete(_) => self.clone(),
        };
        Ok(query.get(executor).await?.into_iter().next())
    }

    pub async fn execute<E>(&self, executor: &E) -> Result<u64>
    where
        E: QueryExecutor,
    {
        match &self.ast.body {
            QueryBody::Select(_) | QueryBody::SetOperation(_) => Err(Error::message(
                "execute() is not available for select queries; use get() or first() instead",
            )),
            QueryBody::Insert(_) | QueryBody::Update(_) | QueryBody::Delete(_) => {
                let compiled = self.compile()?;
                executor
                    .execute_compiled_with(&compiled, self.options.clone())
                    .await
            }
        }
    }

    pub fn stream<'a, E>(&'a self, executor: &'a E) -> Result<DbRecordStream<'a>>
    where
        E: QueryExecutor,
    {
        let compiled = self.compile()?;
        Ok(executor.stream_records(compiled, self.options.clone()))
    }

    pub async fn paginate<E>(
        &self,
        executor: &E,
        pagination: Pagination,
    ) -> Result<Paginated<DbRecord>>
    where
        E: QueryExecutor,
    {
        let total = count_query_ast(executor, &self.ast).await?;
        let data = self
            .clone()
            .limit(pagination.per_page)
            .offset(pagination.offset())
            .get(executor)
            .await?;

        Ok(Paginated {
            data,
            pagination,
            total,
        })
    }

    pub async fn count<E>(&self, executor: &E) -> Result<u64>
    where
        E: QueryExecutor,
    {
        count_query_ast(executor, &self.ast).await
    }

    pub async fn count_distinct<E>(&self, executor: &E, expr: impl Into<Expr>) -> Result<u64>
    where
        E: QueryExecutor,
    {
        Ok(execute_scalar_projection_on_ast(
            executor,
            &self.ast,
            AggregateProjection::<i64>::internal_count_distinct(expr.into()),
        )
        .await? as u64)
    }

    pub async fn sum<E, T>(&self, executor: &E, expr: impl Into<Expr>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        execute_scalar_projection_on_ast(
            executor,
            &self.ast,
            AggregateProjection::<Option<T>>::internal_sum(expr.into()),
        )
        .await
    }

    pub async fn avg<E, T>(&self, executor: &E, expr: impl Into<Expr>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        execute_scalar_projection_on_ast(
            executor,
            &self.ast,
            AggregateProjection::<Option<T>>::internal_avg(expr.into()),
        )
        .await
    }

    pub async fn min<E, T>(&self, executor: &E, expr: impl Into<Expr>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        execute_scalar_projection_on_ast(
            executor,
            &self.ast,
            AggregateProjection::<Option<T>>::internal_min(expr.into()),
        )
        .await
    }

    pub async fn max<E, T>(&self, executor: &E, expr: impl Into<Expr>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        execute_scalar_projection_on_ast(
            executor,
            &self.ast,
            AggregateProjection::<Option<T>>::internal_max(expr.into()),
        )
        .await
    }

    pub(crate) async fn aggregate_over_alias<E, T>(
        &self,
        executor: &E,
        alias: &str,
        projection: AggregateProjection<T>,
    ) -> Result<T>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        let wrapped = wrap_query_for_alias_aggregate(
            &self.ast,
            alias,
            super::ast::DbType::Int64,
            projection.node(),
        );
        let compiled = PostgresCompiler::compile(&wrapped)?;
        let record = executor
            .query_records(&compiled)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| Error::message("aggregate query returned no rows"))?;
        projection.decode(&record)
    }

    pub async fn explain<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(executor, &self.compile()?, false, self.options.clone()).await
    }

    pub async fn explain_analyze<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(executor, &self.compile()?, true, self.options.clone()).await
    }

    fn lock(mut self, strength: LockStrength) -> Self {
        if let QueryBody::Select(select) = &mut self.ast.body {
            let existing = select.lock.take().unwrap_or(LockClause {
                strength,
                of: Vec::new(),
                behavior: LockBehavior::Wait,
            });
            select.lock = Some(LockClause {
                strength,
                ..existing
            });
        }
        self
    }
}

#[derive(Clone)]
pub struct ProjectionQuery<P: 'static> {
    query: Query,
    meta: &'static ProjectionMeta<P>,
}

impl<P> ProjectionQuery<P>
where
    P: Projection,
{
    pub fn table(source: impl Into<FromItem>) -> Self {
        Self::new(source, P::projection_meta())
    }
}

impl<P> ProjectionQuery<P>
where
    P: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(source: impl Into<FromItem>, meta: &'static ProjectionMeta<P>) -> Self {
        Self {
            query: Query::table(source),
            meta,
        }
    }

    pub fn with_cte(mut self, cte: Cte) -> Self {
        self.query = self.query.with_cte(cte);
        self
    }

    pub fn distinct(mut self) -> Self {
        self.query = self.query.distinct();
        self
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.query = self.query.with_timeout(timeout);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.query = self.query.with_label(label);
        self
    }

    pub fn select_field<T>(mut self, field: ProjectionField<P, T>, expr: impl Into<Expr>) -> Self {
        self.query = self.query.select_item(field.select(expr));
        self
    }

    pub fn select_source<T>(mut self, field: ProjectionField<P, T>, table_alias: &str) -> Self {
        self.query = self
            .query
            .select_item(field.select_from(table_alias).unwrap_or_else(|_| {
                field.select(Expr::column(
                    super::ast::ColumnRef::new(table_alias, field.alias()).typed(field.db_type()),
                ))
            }));
        self
    }

    pub fn select_aggregate<T>(mut self, projection: AggregateProjection<T>) -> Self {
        self.query = self.query.select_aggregate(projection);
        self
    }

    pub fn join(mut self, kind: JoinKind, table: impl Into<FromItem>, on: Condition) -> Self {
        self.query = self.query.join(kind, table, on);
        self
    }

    pub fn inner_join(mut self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.query = self.query.inner_join(table, on);
        self
    }

    pub fn left_join(mut self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.query = self.query.left_join(table, on);
        self
    }

    pub fn right_join(mut self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.query = self.query.right_join(table, on);
        self
    }

    pub fn full_outer_join(mut self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.query = self.query.full_outer_join(table, on);
        self
    }

    pub fn cross_join(mut self, table: impl Into<FromItem>) -> Self {
        self.query = self.query.cross_join(table);
        self
    }

    pub fn inner_join_lateral(mut self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.query = self.query.inner_join_lateral(table, on);
        self
    }

    pub fn left_join_lateral(mut self, table: impl Into<FromItem>, on: Condition) -> Self {
        self.query = self.query.left_join_lateral(table, on);
        self
    }

    pub fn cross_join_lateral(mut self, table: impl Into<FromItem>) -> Self {
        self.query = self.query.cross_join_lateral(table);
        self
    }

    pub fn where_(mut self, condition: Condition) -> Self {
        self.query = self.query.where_(condition);
        self
    }

    pub fn group_by(mut self, expr: impl Into<Expr>) -> Self {
        self.query = self.query.group_by(expr);
        self
    }

    pub fn having(mut self, condition: Condition) -> Self {
        self.query = self.query.having(condition);
        self
    }

    pub fn order_by(mut self, order: OrderBy) -> Self {
        self.query = self.query.order_by(order);
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.query = self.query.limit(limit);
        self
    }

    pub fn offset(mut self, offset: u64) -> Self {
        self.query = self.query.offset(offset);
        self
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            query: self.query.union(other.query),
            meta: self.meta,
        }
    }

    pub fn union_all(self, other: Self) -> Self {
        Self {
            query: self.query.union_all(other.query),
            meta: self.meta,
        }
    }

    pub fn ast(&self) -> &QueryAst {
        self.query.ast()
    }

    pub fn to_compiled_sql(&self) -> Result<super::compiler::CompiledSql> {
        self.query.to_compiled_sql()
    }

    pub fn for_update(mut self) -> Self {
        self.query = self.query.for_update();
        self
    }

    pub fn for_no_key_update(mut self) -> Self {
        self.query = self.query.for_no_key_update();
        self
    }

    pub fn for_share(mut self) -> Self {
        self.query = self.query.for_share();
        self
    }

    pub fn for_key_share(mut self) -> Self {
        self.query = self.query.for_key_share();
        self
    }

    pub fn of<I, S>(mut self, aliases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.query = self.query.of(aliases);
        self
    }

    pub fn skip_locked(mut self) -> Self {
        self.query = self.query.skip_locked();
        self
    }

    pub fn nowait(mut self) -> Self {
        self.query = self.query.nowait();
        self
    }

    pub async fn get<E>(&self, executor: &E) -> Result<Vec<P>>
    where
        E: QueryExecutor,
    {
        self.query
            .get(executor)
            .await?
            .iter()
            .map(|record| self.meta.hydrate_record(record))
            .collect()
    }

    pub fn stream<'a, E>(&'a self, executor: &'a E) -> Result<BoxStream<'a, Result<P>>>
    where
        E: QueryExecutor,
    {
        Ok(self
            .query
            .stream(executor)?
            .map(|record| record.and_then(|record| self.meta.hydrate_record(&record)))
            .boxed())
    }

    pub async fn first<E>(&self, executor: &E) -> Result<Option<P>>
    where
        E: QueryExecutor,
    {
        Ok(self.get(executor).await?.into_iter().next())
    }

    pub async fn paginate<E>(&self, executor: &E, pagination: Pagination) -> Result<Paginated<P>>
    where
        E: QueryExecutor,
    {
        let total = self.query.count(executor).await?;
        let data = self
            .clone()
            .limit(pagination.per_page)
            .offset(pagination.offset())
            .get(executor)
            .await?;
        Ok(Paginated {
            data,
            pagination,
            total,
        })
    }

    pub async fn count<E>(&self, executor: &E) -> Result<u64>
    where
        E: QueryExecutor,
    {
        self.query.count(executor).await
    }

    pub async fn count_distinct<E, T>(
        &self,
        executor: &E,
        field: ProjectionField<P, T>,
    ) -> Result<u64>
    where
        E: QueryExecutor,
    {
        Ok(self
            .query
            .aggregate_over_alias(
                executor,
                field.alias(),
                AggregateProjection::<i64>::internal_count_distinct(field.alias()),
            )
            .await? as u64)
    }

    pub async fn sum<E, T>(&self, executor: &E, field: ProjectionField<P, T>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        let alias = field.alias();
        let wrapped = wrap_query_for_alias_aggregate(
            self.query.ast(),
            alias,
            field.db_type(),
            AggregateProjection::<Option<T>>::internal_sum(alias).node(),
        );
        decode_wrapped_projection(
            executor,
            wrapped,
            AggregateProjection::<Option<T>>::internal_sum(alias),
        )
        .await
    }

    pub async fn avg<E, T>(&self, executor: &E, field: ProjectionField<P, T>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        let alias = field.alias();
        let wrapped = wrap_query_for_alias_aggregate(
            self.query.ast(),
            alias,
            field.db_type(),
            AggregateProjection::<Option<T>>::internal_avg(alias).node(),
        );
        decode_wrapped_projection(
            executor,
            wrapped,
            AggregateProjection::<Option<T>>::internal_avg(alias),
        )
        .await
    }

    pub async fn min<E, T>(&self, executor: &E, field: ProjectionField<P, T>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        let alias = field.alias();
        let wrapped = wrap_query_for_alias_aggregate(
            self.query.ast(),
            alias,
            field.db_type(),
            AggregateProjection::<Option<T>>::internal_min(alias).node(),
        );
        decode_wrapped_projection(
            executor,
            wrapped,
            AggregateProjection::<Option<T>>::internal_min(alias),
        )
        .await
    }

    pub async fn max<E, T>(&self, executor: &E, field: ProjectionField<P, T>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        let alias = field.alias();
        let wrapped = wrap_query_for_alias_aggregate(
            self.query.ast(),
            alias,
            field.db_type(),
            AggregateProjection::<Option<T>>::internal_max(alias).node(),
        );
        decode_wrapped_projection(
            executor,
            wrapped,
            AggregateProjection::<Option<T>>::internal_max(alias),
        )
        .await
    }

    pub async fn explain<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        self.query.explain(executor).await
    }

    pub async fn explain_analyze<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        self.query.explain_analyze(executor).await
    }
}

#[derive(Clone)]
pub struct ModelQuery<M: 'static> {
    table: &'static TableMeta<M>,
    with: Vec<CteNode>,
    select: SelectNode,
    relations: Vec<AnyRelation<M>>,
    relation_aggregates: Vec<AnyRelationAggregate<M>>,
    stream_batch_size: usize,
    options: QueryExecutionOptions,
}

impl<M> ModelQuery<M>
where
    M: Model,
{
    pub fn new(table: &'static TableMeta<M>) -> Self {
        Self {
            table,
            with: Vec::new(),
            select: SelectNode {
                from: FromItem::Table(table.table_ref()),
                distinct: false,
                columns: table.all_select_items(),
                joins: Vec::new(),
                condition: None,
                group_by: Vec::new(),
                having: None,
                order_by: Vec::new(),
                limit: None,
                offset: None,
                lock: None,
                relations: Vec::new(),
                aggregates: Vec::new(),
            },
            relations: Vec::new(),
            relation_aggregates: Vec::new(),
            stream_batch_size: 256,
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.options.label = Some(label.into());
        self
    }

    pub fn with_stream_batch_size(mut self, batch_size: usize) -> Self {
        self.stream_batch_size = batch_size.max(1);
        self
    }

    pub fn with_cte(mut self, cte: Cte) -> Self {
        self.with.push(cte.node);
        self
    }

    pub fn where_(mut self, condition: Condition) -> Self {
        self.select.condition = merge_condition(self.select.condition.take(), condition);
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.select.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: u64) -> Self {
        self.select.offset = Some(offset);
        self
    }

    pub fn order_by(mut self, order: OrderBy) -> Self {
        self.select.order_by.push(order);
        self
    }

    pub fn with<To>(mut self, relation: RelationDef<M, To>) -> Self
    where
        To: Model,
    {
        self.select.relations.push(relation.node());
        self.relations.push(std::sync::Arc::new(relation));
        self
    }

    pub fn with_many_to_many<To, Pivot>(mut self, relation: ManyToManyDef<M, To, Pivot>) -> Self
    where
        To: Model,
        Pivot: Clone + Send + Sync + 'static,
    {
        self.select.relations.push(relation.node());
        self.relations.push(std::sync::Arc::new(relation));
        self
    }

    pub fn with_aggregate<Value>(mut self, aggregate: RelationAggregateDef<M, Value>) -> Self {
        self.select.relations.push(aggregate.node());
        self.relation_aggregates.push(aggregate.into_loader());
        self
    }

    pub(crate) fn with_aggregate_boxed(mut self, aggregate: AnyRelationAggregate<M>) -> Self {
        self.select.relations.push(aggregate.node());
        self.relation_aggregates.push(aggregate);
        self
    }

    pub(crate) fn with_boxed(mut self, relation: AnyRelation<M>) -> Self {
        self.select.relations.push(relation.node());
        self.relations.push(relation);
        self
    }

    pub fn where_has<To, F>(mut self, relation: RelationDef<M, To>, scope: F) -> Self
    where
        To: Model,
        F: FnOnce(ModelQuery<To>) -> ModelQuery<To>,
    {
        let scoped = scope(ModelQuery::new(To::table_meta()));
        let relation = relation.scoped_with_filter(scoped.select.condition.clone());
        self.select.condition =
            merge_condition(self.select.condition.take(), relation.exists_condition());
        self
    }

    pub fn where_has_many_to_many<To, Pivot, F>(
        mut self,
        relation: ManyToManyDef<M, To, Pivot>,
        scope: F,
    ) -> Self
    where
        To: Model,
        Pivot: Clone + Send + Sync + 'static,
        F: FnOnce(ModelQuery<To>) -> ModelQuery<To>,
    {
        let scoped = scope(ModelQuery::new(To::table_meta()));
        let relation = relation.scoped_with_filter(scoped.select.condition.clone());
        self.select.condition =
            merge_condition(self.select.condition.take(), relation.exists_condition());
        self
    }

    pub fn ast(&self) -> QueryAst {
        QueryAst {
            with: self.with.clone(),
            body: QueryBody::Select(Box::new(self.select.clone())),
        }
    }

    pub fn to_compiled_sql(&self) -> Result<super::compiler::CompiledSql> {
        PostgresCompiler::compile(&self.ast())
    }

    pub fn for_update(mut self) -> Self {
        self = self.lock(LockStrength::Update);
        self
    }

    pub fn for_no_key_update(mut self) -> Self {
        self = self.lock(LockStrength::NoKeyUpdate);
        self
    }

    pub fn for_share(mut self) -> Self {
        self = self.lock(LockStrength::Share);
        self
    }

    pub fn for_key_share(mut self) -> Self {
        self = self.lock(LockStrength::KeyShare);
        self
    }

    pub fn of<I, S>(mut self, aliases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let lock = self.select.lock.get_or_insert(LockClause {
            strength: LockStrength::Update,
            of: Vec::new(),
            behavior: LockBehavior::Wait,
        });
        lock.of.extend(aliases.into_iter().map(Into::into));
        self
    }

    pub fn skip_locked(mut self) -> Self {
        let lock = self.select.lock.get_or_insert(LockClause {
            strength: LockStrength::Update,
            of: Vec::new(),
            behavior: LockBehavior::Wait,
        });
        lock.behavior = LockBehavior::SkipLocked;
        self
    }

    pub fn nowait(mut self) -> Self {
        let lock = self.select.lock.get_or_insert(LockClause {
            strength: LockStrength::Update,
            of: Vec::new(),
            behavior: LockBehavior::Wait,
        });
        lock.behavior = LockBehavior::NoWait;
        self
    }

    pub async fn get<E>(&self, executor: &E) -> Result<Vec<M>>
    where
        E: QueryExecutor,
    {
        let mut entries = self.fetch_entries_dyn(executor).await?;
        Ok(entries.drain(..).map(|(_, model)| model).collect())
    }

    pub fn stream<'a, E>(&'a self, executor: &'a E) -> Result<BoxStream<'a, Result<M>>>
    where
        E: QueryExecutor,
    {
        let compiled = self.to_compiled_sql()?;
        Ok(model_query_stream(ModelStreamState {
            executor,
            root_stream: executor.stream_records(compiled, self.options.clone()),
            table: self.table,
            relations: self.relations.clone(),
            relation_aggregates: self.relation_aggregates.clone(),
            stream_batch_size: self.stream_batch_size.max(1),
            buffered: VecDeque::new(),
            pending_error: None,
            finished: false,
            options: self.options.clone(),
        }))
    }

    pub async fn first<E>(&self, executor: &E) -> Result<Option<M>>
    where
        E: QueryExecutor,
    {
        Ok(self
            .clone()
            .limit(1)
            .get(executor)
            .await?
            .into_iter()
            .next())
    }

    pub async fn paginate<E>(&self, executor: &E, pagination: Pagination) -> Result<Paginated<M>>
    where
        E: QueryExecutor,
    {
        let total = count_query_ast(executor, &self.ast()).await?;
        let data = self
            .clone()
            .limit(pagination.per_page)
            .offset(pagination.offset())
            .get(executor)
            .await?;
        Ok(Paginated {
            data,
            pagination,
            total,
        })
    }

    pub async fn count<E>(&self, executor: &E) -> Result<u64>
    where
        E: QueryExecutor,
    {
        count_query_ast(executor, &self.ast()).await
    }

    pub async fn count_distinct<E, T>(&self, executor: &E, column: Column<M, T>) -> Result<u64>
    where
        E: QueryExecutor,
    {
        Ok(execute_scalar_projection_on_ast(
            executor,
            &self.ast(),
            AggregateProjection::<i64>::internal_count_distinct(column.column_ref()),
        )
        .await? as u64)
    }

    pub async fn sum<E, T>(&self, executor: &E, column: Column<M, T>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        execute_scalar_projection_on_ast(
            executor,
            &self.ast(),
            AggregateProjection::<Option<T>>::internal_sum(column.column_ref()),
        )
        .await
    }

    pub async fn avg<E, T>(&self, executor: &E, column: Column<M, T>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        execute_scalar_projection_on_ast(
            executor,
            &self.ast(),
            AggregateProjection::<Option<T>>::internal_avg(column.column_ref()),
        )
        .await
    }

    pub async fn min<E, T>(&self, executor: &E, column: Column<M, T>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        execute_scalar_projection_on_ast(
            executor,
            &self.ast(),
            AggregateProjection::<Option<T>>::internal_min(column.column_ref()),
        )
        .await
    }

    pub async fn max<E, T>(&self, executor: &E, column: Column<M, T>) -> Result<Option<T>>
    where
        E: QueryExecutor,
        T: FromDbValue,
    {
        execute_scalar_projection_on_ast(
            executor,
            &self.ast(),
            AggregateProjection::<Option<T>>::internal_max(column.column_ref()),
        )
        .await
    }

    pub(crate) async fn fetch_entries_dyn(
        &self,
        executor: &dyn QueryExecutor,
    ) -> Result<Vec<(DbRecord, M)>> {
        let compiled = PostgresCompiler::compile(&self.ast())?;
        let records = executor
            .query_records_with(&compiled, self.options.clone())
            .await?;
        let models = hydrate_model_batch(
            executor,
            self.table,
            &self.relations,
            &self.relation_aggregates,
            &records,
            &self.options,
        )
        .await?;

        Ok(records.into_iter().zip(models.into_iter()).collect())
    }

    pub async fn explain<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            false,
            self.options.clone(),
        )
        .await
    }

    pub async fn explain_analyze<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            true,
            self.options.clone(),
        )
        .await
    }

    fn lock(mut self, strength: LockStrength) -> Self {
        let existing = self.select.lock.take().unwrap_or(LockClause {
            strength,
            of: Vec::new(),
            behavior: LockBehavior::Wait,
        });
        self.select.lock = Some(LockClause {
            strength,
            ..existing
        });
        self
    }
}

#[derive(Clone)]
pub struct CreateModel<M: 'static> {
    table: &'static TableMeta<M>,
    rows: Vec<Vec<(super::ast::ColumnRef, Expr)>>,
    on_conflict: Option<OnConflictNode>,
    options: QueryExecutionOptions,
}

#[derive(Clone)]
pub struct CreateManyModel<M: 'static> {
    table: &'static TableMeta<M>,
    rows: Vec<Vec<(super::ast::ColumnRef, Expr)>>,
    on_conflict: Option<OnConflictNode>,
    without_lifecycle: bool,
    options: QueryExecutionOptions,
}

pub struct CreateRow<M: 'static> {
    values: Vec<(super::ast::ColumnRef, Expr)>,
    _marker: PhantomData<fn() -> M>,
}

impl<M> CreateRow<M> {
    fn new() -> Self {
        Self {
            values: Vec::new(),
            _marker: PhantomData,
        }
    }

    fn into_values(self) -> Vec<(super::ast::ColumnRef, Expr)> {
        self.values
    }

    pub fn set<T, V>(mut self, column: Column<M, T>, value: V) -> Self
    where
        V: IntoFieldValue<T>,
    {
        self.values.push((
            column.column_ref(),
            Expr::value(value.into_field_value(column.db_type())),
        ));
        self
    }

    pub fn set_expr<T>(mut self, column: Column<M, T>, expr: impl Into<Expr>) -> Self {
        self.values.push((column.column_ref(), expr.into()));
        self
    }

    pub fn set_null<T>(mut self, column: Column<M, T>) -> Self {
        self.values.push((
            column.column_ref(),
            Expr::value(super::ast::DbValue::Null(column.db_type())),
        ));
        self
    }
}

impl<M> CreateModel<M>
where
    M: Model,
{
    pub(crate) fn new(table: &'static TableMeta<M>) -> Self {
        Self {
            table,
            rows: Vec::new(),
            on_conflict: None,
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.options.label = Some(label.into());
        self
    }

    pub fn set<T, V>(mut self, column: Column<M, T>, value: V) -> Self
    where
        V: IntoFieldValue<T>,
    {
        ensure_insert_row(&mut self.rows).push((
            column.column_ref(),
            Expr::value(value.into_field_value(column.db_type())),
        ));
        self
    }

    pub fn set_expr<T>(mut self, column: Column<M, T>, expr: impl Into<Expr>) -> Self {
        ensure_insert_row(&mut self.rows).push((column.column_ref(), expr.into()));
        self
    }

    pub fn on_conflict_columns<I, C>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = C>,
        C: Into<super::ast::ColumnRef>,
    {
        self.on_conflict = Some(OnConflictNode {
            target: Some(OnConflictTarget::Columns(
                columns.into_iter().map(Into::into).collect(),
            )),
            action: current_conflict_action(self.on_conflict.take()),
        });
        self
    }

    pub fn on_conflict_constraint(mut self, constraint: impl Into<String>) -> Self {
        self.on_conflict = Some(OnConflictNode {
            target: Some(OnConflictTarget::Constraint(constraint.into())),
            action: current_conflict_action(self.on_conflict.take()),
        });
        self
    }

    pub fn do_nothing(mut self) -> Self {
        upsert_node_model(&mut self.on_conflict).action = OnConflictAction::DoNothing;
        self
    }

    pub fn do_update(mut self) -> Self {
        upsert_node_model(&mut self.on_conflict).action =
            OnConflictAction::DoUpdate(Box::new(OnConflictUpdate {
                assignments: Vec::new(),
                condition: None,
            }));
        self
    }

    pub fn set_conflict<T, V>(mut self, column: Column<M, T>, value: V) -> Self
    where
        V: IntoFieldValue<T>,
    {
        let db_type = column.db_type();
        let column_ref = column.column_ref();
        self = self.set_conflict_expr(column_ref, Expr::value(value.into_field_value(db_type)));
        self
    }

    pub fn set_conflict_expr(
        mut self,
        column: impl Into<super::ast::ColumnRef>,
        expr: impl Into<Expr>,
    ) -> Self {
        if let Some(OnConflictNode {
            action: OnConflictAction::DoUpdate(conflict),
            ..
        }) = &mut self.on_conflict
        {
            conflict.assignments.push((column.into(), expr.into()));
        }
        self
    }

    pub fn set_excluded<T>(mut self, column: Column<M, T>) -> Self {
        let column_ref = column.column_ref();
        self = self.set_conflict_expr(column_ref.clone(), Expr::excluded(column_ref));
        self
    }

    pub fn where_(mut self, condition: Condition) -> Self {
        if let Some(OnConflictNode {
            action: OnConflictAction::DoUpdate(conflict),
            ..
        }) = &mut self.on_conflict
        {
            conflict.condition = merge_condition(conflict.condition.take(), condition);
        }
        self
    }

    fn ast(&self, returning_all: bool) -> QueryAst {
        QueryAst::insert(InsertNode {
            into: self.table.table_ref(),
            source: InsertSource::Values(self.rows.clone()),
            on_conflict: self.on_conflict.clone(),
            returning: if returning_all {
                self.table.all_select_items()
            } else {
                Vec::new()
            },
        })
    }

    fn validate_rows(&self) -> Result<()> {
        if self.rows.len() != 1 {
            return Err(Error::message(
                "create() expects exactly one row; use create_many() for bulk inserts",
            ));
        }

        if self.rows[0].is_empty() {
            return Err(Error::message(
                "create() requires at least one assigned column before save() or execute()",
            ));
        }

        Ok(())
    }

    fn compiled_sql(&self, returning_all: bool) -> Result<super::compiler::CompiledSql> {
        self.validate_rows()?;
        PostgresCompiler::compile(&self.ast(returning_all))
    }

    pub async fn execute<E>(&self, executor: &E) -> Result<u64>
    where
        E: ModelWriteExecutor,
    {
        Ok(create_model_records(self, executor).await?.len() as u64)
    }

    pub async fn save<E>(&self, executor: &E) -> Result<M>
    where
        E: ModelWriteExecutor,
    {
        let mut records = self.get(executor).await?;
        match records.len() {
            1 => Ok(records.remove(0)),
            0 => Err(Error::message("create() did not return a record")),
            _ => Err(Error::message(
                "create() returned more than one record; use get() instead",
            )),
        }
    }

    pub async fn get<E>(&self, executor: &E) -> Result<Vec<M>>
    where
        E: ModelWriteExecutor,
    {
        create_model_records(self, executor).await
    }

    pub async fn first<E>(&self, executor: &E) -> Result<Option<M>>
    where
        E: ModelWriteExecutor,
    {
        Ok(self.get(executor).await?.into_iter().next())
    }

    pub fn to_compiled_sql(&self) -> Result<super::compiler::CompiledSql> {
        self.compiled_sql(true)
    }

    pub async fn explain<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            false,
            self.options.clone(),
        )
        .await
    }

    pub async fn explain_analyze<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            true,
            self.options.clone(),
        )
        .await
    }
}

impl<M> CreateManyModel<M>
where
    M: Model,
{
    pub(crate) fn new(table: &'static TableMeta<M>) -> Self {
        Self {
            table,
            rows: Vec::new(),
            on_conflict: None,
            without_lifecycle: false,
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.options.label = Some(label.into());
        self
    }

    pub fn row<F>(mut self, build: F) -> Self
    where
        F: FnOnce(CreateRow<M>) -> CreateRow<M>,
    {
        self.rows.push(build(CreateRow::new()).into_values());
        self
    }

    pub fn on_conflict_columns<I, C>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = C>,
        C: Into<super::ast::ColumnRef>,
    {
        self.on_conflict = Some(OnConflictNode {
            target: Some(OnConflictTarget::Columns(
                columns.into_iter().map(Into::into).collect(),
            )),
            action: current_conflict_action(self.on_conflict.take()),
        });
        self
    }

    pub fn on_conflict_constraint(mut self, constraint: impl Into<String>) -> Self {
        self.on_conflict = Some(OnConflictNode {
            target: Some(OnConflictTarget::Constraint(constraint.into())),
            action: current_conflict_action(self.on_conflict.take()),
        });
        self
    }

    pub fn do_nothing(mut self) -> Self {
        upsert_node_model(&mut self.on_conflict).action = OnConflictAction::DoNothing;
        self
    }

    pub fn do_update(mut self) -> Self {
        upsert_node_model(&mut self.on_conflict).action =
            OnConflictAction::DoUpdate(Box::new(OnConflictUpdate {
                assignments: Vec::new(),
                condition: None,
            }));
        self
    }

    pub fn set_conflict<T, V>(mut self, column: Column<M, T>, value: V) -> Self
    where
        V: IntoFieldValue<T>,
    {
        let db_type = column.db_type();
        let column_ref = column.column_ref();
        self = self.set_conflict_expr(column_ref, Expr::value(value.into_field_value(db_type)));
        self
    }

    pub fn set_conflict_expr(
        mut self,
        column: impl Into<super::ast::ColumnRef>,
        expr: impl Into<Expr>,
    ) -> Self {
        if let Some(OnConflictNode {
            action: OnConflictAction::DoUpdate(conflict),
            ..
        }) = &mut self.on_conflict
        {
            conflict.assignments.push((column.into(), expr.into()));
        }
        self
    }

    pub fn set_excluded<T>(mut self, column: Column<M, T>) -> Self {
        let column_ref = column.column_ref();
        self = self.set_conflict_expr(column_ref.clone(), Expr::excluded(column_ref));
        self
    }

    pub fn where_(mut self, condition: Condition) -> Self {
        if let Some(OnConflictNode {
            action: OnConflictAction::DoUpdate(conflict),
            ..
        }) = &mut self.on_conflict
        {
            conflict.condition = merge_condition(conflict.condition.take(), condition);
        }
        self
    }

    fn ast(&self, returning_all: bool) -> QueryAst {
        QueryAst::insert(InsertNode {
            into: self.table.table_ref(),
            source: InsertSource::Values(self.rows.clone()),
            on_conflict: self.on_conflict.clone(),
            returning: if returning_all {
                self.table.all_select_items()
            } else {
                Vec::new()
            },
        })
    }

    fn validate_rows(&self) -> Result<()> {
        if self.rows.is_empty() {
            return Err(Error::message(
                "create_many() requires at least one row before execute() or get()",
            ));
        }

        if self.rows.iter().any(Vec::is_empty) {
            return Err(Error::message(
                "create_many() does not allow empty rows; each row needs at least one assigned column",
            ));
        }

        Ok(())
    }

    fn compiled_sql(&self, returning_all: bool) -> Result<super::compiler::CompiledSql> {
        self.validate_rows()?;
        PostgresCompiler::compile(&self.ast(returning_all))
    }

    async fn fast_execute<E>(&self, executor: &E) -> Result<u64>
    where
        E: QueryExecutor,
    {
        executor
            .execute_compiled_with(&self.compiled_sql(false)?, self.options.clone())
            .await
    }

    async fn fast_get<E>(&self, executor: &E) -> Result<Vec<M>>
    where
        E: QueryExecutor,
    {
        let records = executor
            .query_records_with(&self.compiled_sql(true)?, self.options.clone())
            .await?;
        records
            .iter()
            .map(|record| self.table.hydrate_record(record))
            .collect()
    }

    pub async fn execute<E>(&self, executor: &E) -> Result<u64>
    where
        E: ModelWriteExecutor,
    {
        if self.without_lifecycle {
            self.fast_execute(executor).await
        } else {
            Ok(create_many_model_records(self, executor).await?.len() as u64)
        }
    }

    pub async fn get<E>(&self, executor: &E) -> Result<Vec<M>>
    where
        E: ModelWriteExecutor,
    {
        if self.without_lifecycle {
            self.fast_get(executor).await
        } else {
            create_many_model_records(self, executor).await
        }
    }

    pub async fn first<E>(&self, executor: &E) -> Result<Option<M>>
    where
        E: ModelWriteExecutor,
    {
        Ok(self.get(executor).await?.into_iter().next())
    }

    pub fn to_compiled_sql(&self) -> Result<super::compiler::CompiledSql> {
        self.compiled_sql(true)
    }

    pub async fn explain<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            false,
            self.options.clone(),
        )
        .await
    }

    pub async fn explain_analyze<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            true,
            self.options.clone(),
        )
        .await
    }
}

#[derive(Clone)]
pub struct UpdateModel<M: 'static> {
    table: &'static TableMeta<M>,
    values: Vec<(super::ast::ColumnRef, Expr)>,
    from: Vec<FromItem>,
    condition: Option<Condition>,
    allow_all: bool,
    without_lifecycle: bool,
    options: QueryExecutionOptions,
}

impl<M> UpdateModel<M>
where
    M: Model,
{
    pub(crate) fn new(table: &'static TableMeta<M>) -> Self {
        Self {
            table,
            values: Vec::new(),
            from: Vec::new(),
            condition: None,
            allow_all: false,
            without_lifecycle: false,
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.options.label = Some(label.into());
        self
    }

    pub fn set<T, V>(mut self, column: Column<M, T>, value: V) -> Self
    where
        V: IntoFieldValue<T>,
    {
        self.values.push((
            column.column_ref(),
            Expr::value(value.into_field_value(column.db_type())),
        ));
        self
    }

    pub fn set_expr<T>(mut self, column: Column<M, T>, expr: impl Into<Expr>) -> Self {
        self.values.push((column.column_ref(), expr.into()));
        self
    }

    pub fn set_null<T>(mut self, column: Column<M, T>) -> Self {
        self.values.push((
            column.column_ref(),
            Expr::value(super::ast::DbValue::Null(column.db_type())),
        ));
        self
    }

    pub fn where_(mut self, condition: Condition) -> Self {
        self.condition = merge_condition(self.condition.take(), condition);
        self
    }

    pub fn from(mut self, source: impl Into<FromItem>) -> Self {
        self.from.push(source.into());
        self
    }

    pub fn allow_all(mut self) -> Self {
        self.allow_all = true;
        self
    }

    pub fn without_lifecycle(mut self) -> Self {
        self.without_lifecycle = true;
        self
    }

    fn ast(&self, returning_all: bool) -> QueryAst {
        QueryAst::update(UpdateNode {
            table: self.table.table_ref(),
            values: self.values.clone(),
            from: self.from.clone(),
            condition: self.condition.clone(),
            returning: if returning_all {
                self.table.all_select_items()
            } else {
                Vec::new()
            },
        })
    }

    fn validate(&self) -> Result<()> {
        if self.values.is_empty() {
            return Err(Error::message(
                "update() requires at least one assigned column before save() or execute()",
            ));
        }

        if self.condition.is_none() && !self.allow_all {
            return Err(Error::message(
                "update() requires a where clause; call allow_all() to update every row explicitly",
            ));
        }

        Ok(())
    }

    fn compiled_sql(&self, returning_all: bool) -> Result<super::compiler::CompiledSql> {
        self.validate()?;
        PostgresCompiler::compile(&self.ast(returning_all))
    }

    async fn fast_execute<E>(&self, executor: &E) -> Result<u64>
    where
        E: QueryExecutor,
    {
        executor
            .execute_compiled_with(&self.compiled_sql(false)?, self.options.clone())
            .await
    }

    async fn fast_get<E>(&self, executor: &E) -> Result<Vec<M>>
    where
        E: QueryExecutor,
    {
        let records = executor
            .query_records_with(&self.compiled_sql(true)?, self.options.clone())
            .await?;
        records
            .iter()
            .map(|record| self.table.hydrate_record(record))
            .collect()
    }

    pub async fn execute<E>(&self, executor: &E) -> Result<u64>
    where
        E: ModelWriteExecutor,
    {
        if self.without_lifecycle {
            self.fast_execute(executor).await
        } else {
            Ok(update_model_records(self, executor).await?.len() as u64)
        }
    }

    pub async fn save<E>(&self, executor: &E) -> Result<M>
    where
        E: ModelWriteExecutor,
    {
        let mut records = self.get(executor).await?;
        match records.len() {
            1 => Ok(records.remove(0)),
            0 => Err(Error::message("update() did not return a record")),
            _ => Err(Error::message(
                "update() returned more than one record; use get() instead",
            )),
        }
    }

    pub async fn get<E>(&self, executor: &E) -> Result<Vec<M>>
    where
        E: ModelWriteExecutor,
    {
        if self.without_lifecycle {
            self.fast_get(executor).await
        } else {
            update_model_records(self, executor).await
        }
    }

    pub async fn first<E>(&self, executor: &E) -> Result<Option<M>>
    where
        E: ModelWriteExecutor,
    {
        Ok(self.get(executor).await?.into_iter().next())
    }

    pub fn to_compiled_sql(&self) -> Result<super::compiler::CompiledSql> {
        self.compiled_sql(true)
    }

    pub async fn explain<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            false,
            self.options.clone(),
        )
        .await
    }

    pub async fn explain_analyze<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            true,
            self.options.clone(),
        )
        .await
    }
}

#[derive(Clone)]
pub struct DeleteModel<M: 'static> {
    table: &'static TableMeta<M>,
    using: Vec<FromItem>,
    condition: Option<Condition>,
    allow_all: bool,
    without_lifecycle: bool,
    options: QueryExecutionOptions,
}

impl<M> DeleteModel<M>
where
    M: Model,
{
    pub(crate) fn new(table: &'static TableMeta<M>) -> Self {
        Self {
            table,
            using: Vec::new(),
            condition: None,
            allow_all: false,
            without_lifecycle: false,
            options: QueryExecutionOptions::default(),
        }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.options.label = Some(label.into());
        self
    }

    pub fn where_(mut self, condition: Condition) -> Self {
        self.condition = merge_condition(self.condition.take(), condition);
        self
    }

    pub fn using(mut self, source: impl Into<FromItem>) -> Self {
        self.using.push(source.into());
        self
    }

    pub fn allow_all(mut self) -> Self {
        self.allow_all = true;
        self
    }

    pub fn without_lifecycle(mut self) -> Self {
        self.without_lifecycle = true;
        self
    }

    fn ast(&self) -> QueryAst {
        QueryAst::delete(super::ast::DeleteNode {
            from: self.table.table_ref(),
            using: self.using.clone(),
            condition: self.condition.clone(),
            returning: Vec::new(),
        })
    }

    fn validate(&self) -> Result<()> {
        if self.condition.is_none() && !self.allow_all {
            return Err(Error::message(
                "delete() requires a where clause; call allow_all() to delete every row explicitly",
            ));
        }

        Ok(())
    }

    fn compiled_sql(&self) -> Result<super::compiler::CompiledSql> {
        self.validate()?;
        PostgresCompiler::compile(&self.ast())
    }

    async fn fast_execute<E>(&self, executor: &E) -> Result<u64>
    where
        E: QueryExecutor,
    {
        executor
            .execute_compiled_with(&self.compiled_sql()?, self.options.clone())
            .await
    }

    pub async fn execute<E>(&self, executor: &E) -> Result<u64>
    where
        E: ModelWriteExecutor,
    {
        if self.without_lifecycle {
            self.fast_execute(executor).await
        } else {
            delete_model_rows(self, executor).await
        }
    }

    pub fn to_compiled_sql(&self) -> Result<super::compiler::CompiledSql> {
        self.compiled_sql()
    }

    pub async fn explain<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            false,
            self.options.clone(),
        )
        .await
    }

    pub async fn explain_analyze<E>(&self, executor: &E) -> Result<Vec<String>>
    where
        E: QueryExecutor,
    {
        explain_query(
            executor,
            &self.to_compiled_sql()?,
            true,
            self.options.clone(),
        )
        .await
    }
}

type ModelWriteFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

async fn with_model_write_transaction<E, T>(
    executor: &E,
    operation: impl for<'a> FnOnce(
        &'a AppContext,
        &'a super::runtime::DatabaseTransaction,
    ) -> ModelWriteFuture<'a, T>,
) -> Result<T>
where
    E: ModelWriteExecutor,
{
    if let Some(transaction) = executor.active_transaction() {
        return operation(executor.app_context(), transaction).await;
    }

    let transaction = executor.app_context().begin_transaction().await?;
    let result = operation(transaction.app(), transaction.transaction()).await;
    match result {
        Ok(value) => {
            transaction.commit().await?;
            Ok(value)
        }
        Err(error) => {
            let rollback_result = transaction.rollback().await;
            if let Err(rollback_error) = rollback_result {
                return Err(Error::message(format!(
                    "{error}; rollback failed: {rollback_error}"
                )));
            }
            Err(error)
        }
    }
}

async fn create_model_records<E, M>(create: &CreateModel<M>, executor: &E) -> Result<Vec<M>>
where
    E: ModelWriteExecutor,
    M: Model,
{
    create.validate_rows()?;
    let create = create.clone();
    with_model_write_transaction(executor, |app, transaction| {
        Box::pin(async move {
            let database = app.database()?;
            let mut draft = CreateDraft::<M>::new(create.rows[0].clone());
            let context = ModelHookContext::new(app, database, transaction);
            M::Lifecycle::creating(&context, &mut draft).await?;
            context
                .dispatch(ModelCreatingEvent {
                    snapshot: ModelLifecycleSnapshot::for_model::<M>(
                        None,
                        None,
                        Some(draft.pending_record()),
                    ),
                })
                .await?;

            let records = transaction
                .query_records_with(
                    &CreateModel {
                        table: create.table,
                        rows: vec![draft.into_values()],
                        on_conflict: create.on_conflict.clone(),
                        options: create.options.clone(),
                    }
                    .compiled_sql(true)?,
                    create.options.clone(),
                )
                .await?;

            let mut models = Vec::with_capacity(records.len());
            for record in &records {
                let model = create.table.hydrate_record(record)?;
                M::Lifecycle::created(&context, &model, record).await?;
                context
                    .dispatch(ModelCreatedEvent {
                        snapshot: ModelLifecycleSnapshot::for_model::<M>(
                            None,
                            Some(record.clone()),
                            None,
                        ),
                    })
                    .await?;
                models.push(model);
            }

            Ok(models)
        })
    })
    .await
}

async fn create_many_model_records<E, M>(
    create_many: &CreateManyModel<M>,
    executor: &E,
) -> Result<Vec<M>>
where
    E: ModelWriteExecutor,
    M: Model,
{
    create_many.validate_rows()?;
    let create_many = create_many.clone();
    with_model_write_transaction(executor, |app, transaction| {
        Box::pin(async move {
            let mut created = Vec::new();
            for row in &create_many.rows {
                let create = CreateModel {
                    table: create_many.table,
                    rows: vec![row.clone()],
                    on_conflict: create_many.on_conflict.clone(),
                    options: create_many.options.clone(),
                };
                created
                    .extend(create_model_records_in_transaction(&create, app, transaction).await?);
            }
            Ok(created)
        })
    })
    .await
}

async fn create_model_records_in_transaction<M>(
    create: &CreateModel<M>,
    app: &AppContext,
    transaction: &super::runtime::DatabaseTransaction,
) -> Result<Vec<M>>
where
    M: Model,
{
    let database = app.database()?;
    let mut draft = CreateDraft::<M>::new(create.rows[0].clone());
    let context = ModelHookContext::new(app, database, transaction);
    M::Lifecycle::creating(&context, &mut draft).await?;
    context
        .dispatch(ModelCreatingEvent {
            snapshot: ModelLifecycleSnapshot::for_model::<M>(
                None,
                None,
                Some(draft.pending_record()),
            ),
        })
        .await?;

    let records = transaction
        .query_records_with(
            &CreateModel {
                table: create.table,
                rows: vec![draft.into_values()],
                on_conflict: create.on_conflict.clone(),
                options: create.options.clone(),
            }
            .compiled_sql(true)?,
            create.options.clone(),
        )
        .await?;

    let mut models = Vec::with_capacity(records.len());
    for record in &records {
        let model = create.table.hydrate_record(record)?;
        M::Lifecycle::created(&context, &model, record).await?;
        context
            .dispatch(ModelCreatedEvent {
                snapshot: ModelLifecycleSnapshot::for_model::<M>(None, Some(record.clone()), None),
            })
            .await?;
        models.push(model);
    }

    Ok(models)
}

async fn update_model_records<E, M>(update: &UpdateModel<M>, executor: &E) -> Result<Vec<M>>
where
    E: ModelWriteExecutor,
    M: Model,
{
    update.validate()?;
    let update = update.clone();
    with_model_write_transaction(executor, |app, transaction| {
        Box::pin(
            async move { update_model_records_in_transaction(&update, app, transaction).await },
        )
    })
    .await
}

async fn update_model_records_in_transaction<M>(
    update: &UpdateModel<M>,
    app: &AppContext,
    transaction: &super::runtime::DatabaseTransaction,
) -> Result<Vec<M>>
where
    M: Model,
{
    let current_records = select_update_target_records(update, transaction).await?;
    let database = app.database()?;
    let context = ModelHookContext::new(app, database, transaction);
    let mut updated_models = Vec::with_capacity(current_records.len());

    for current_record in current_records {
        let current_model = update.table.hydrate_record(&current_record)?;
        let mut draft = UpdateDraft::<M>::new(update.values.clone());
        M::Lifecycle::updating(&context, &current_model, &mut draft).await?;
        context
            .dispatch(ModelUpdatingEvent {
                snapshot: ModelLifecycleSnapshot::for_model::<M>(
                    Some(current_record.clone()),
                    None,
                    Some(draft.pending_record()),
                ),
            })
            .await?;

        let pk_condition = record_primary_key_condition(update.table, &current_record)?;
        let records = transaction
            .query_records_with(
                &UpdateModel {
                    table: update.table,
                    values: draft.into_values(),
                    from: update.from.clone(),
                    condition: merge_optional_condition(
                        Some(pk_condition),
                        update.condition.clone(),
                    ),
                    allow_all: false,
                    without_lifecycle: false,
                    options: update.options.clone(),
                }
                .compiled_sql(true)?,
                update.options.clone(),
            )
            .await?;

        let after_record = expect_single_record("update()", records)?;
        let after_model = update.table.hydrate_record(&after_record)?;
        M::Lifecycle::updated(
            &context,
            &current_model,
            &after_model,
            &current_record,
            &after_record,
        )
        .await?;
        context
            .dispatch(ModelUpdatedEvent {
                snapshot: ModelLifecycleSnapshot::for_model::<M>(
                    Some(current_record),
                    Some(after_record.clone()),
                    None,
                ),
            })
            .await?;
        updated_models.push(after_model);
    }

    Ok(updated_models)
}

async fn delete_model_rows<E, M>(delete: &DeleteModel<M>, executor: &E) -> Result<u64>
where
    E: ModelWriteExecutor,
    M: Model,
{
    delete.validate()?;
    let delete = delete.clone();
    with_model_write_transaction(executor, |app, transaction| {
        Box::pin(async move { delete_model_rows_in_transaction(&delete, app, transaction).await })
    })
    .await
}

async fn delete_model_rows_in_transaction<M>(
    delete: &DeleteModel<M>,
    app: &AppContext,
    transaction: &super::runtime::DatabaseTransaction,
) -> Result<u64>
where
    M: Model,
{
    let current_records = select_delete_target_records(delete, transaction).await?;
    let database = app.database()?;
    let context = ModelHookContext::new(app, database, transaction);

    for current_record in &current_records {
        let current_model = delete.table.hydrate_record(current_record)?;
        M::Lifecycle::deleting(&context, &current_model, current_record).await?;
        context
            .dispatch(ModelDeletingEvent {
                snapshot: ModelLifecycleSnapshot::for_model::<M>(
                    Some(current_record.clone()),
                    None,
                    None,
                ),
            })
            .await?;

        let pk_condition = record_primary_key_condition(delete.table, current_record)?;
        transaction
            .execute_compiled_with(
                &DeleteModel {
                    table: delete.table,
                    using: delete.using.clone(),
                    condition: merge_optional_condition(
                        Some(pk_condition),
                        delete.condition.clone(),
                    ),
                    allow_all: false,
                    without_lifecycle: false,
                    options: delete.options.clone(),
                }
                .compiled_sql()?,
                delete.options.clone(),
            )
            .await?;

        M::Lifecycle::deleted(&context, &current_model, current_record).await?;
        context
            .dispatch(ModelDeletedEvent {
                snapshot: ModelLifecycleSnapshot::for_model::<M>(
                    Some(current_record.clone()),
                    None,
                    None,
                ),
            })
            .await?;
    }

    Ok(current_records.len() as u64)
}

async fn select_update_target_records<M>(
    update: &UpdateModel<M>,
    executor: &super::runtime::DatabaseTransaction,
) -> Result<Vec<DbRecord>>
where
    M: Model,
{
    let mut query = Query::table(update.table.table_ref());
    for item in update.table.all_select_items() {
        query = query.select_item(item);
    }
    for from in update.from.clone() {
        query = query.cross_join(from);
    }
    if let Some(condition) = update.condition.clone() {
        query = query.where_(condition);
    }
    query = query
        .order_by(OrderBy::asc(update.table.primary_key_ref()))
        .for_update()
        .of([update.table.name()]);
    query.get(executor).await
}

async fn select_delete_target_records<M>(
    delete: &DeleteModel<M>,
    executor: &super::runtime::DatabaseTransaction,
) -> Result<Vec<DbRecord>>
where
    M: Model,
{
    let mut query = Query::table(delete.table.table_ref());
    for item in delete.table.all_select_items() {
        query = query.select_item(item);
    }
    for using in delete.using.clone() {
        query = query.cross_join(using);
    }
    if let Some(condition) = delete.condition.clone() {
        query = query.where_(condition);
    }
    query = query
        .order_by(OrderBy::asc(delete.table.primary_key_ref()))
        .for_update()
        .of([delete.table.name()]);
    query.get(executor).await
}

fn record_primary_key_condition<M>(table: &TableMeta<M>, record: &DbRecord) -> Result<Condition> {
    let primary_key = table.primary_key_column_info().ok_or_else(|| {
        Error::message(format!(
            "missing primary key column `{}` on table `{}`",
            table.primary_key_name(),
            table.name()
        ))
    })?;
    let value = record.get(primary_key.name).cloned().ok_or_else(|| {
        Error::message(format!(
            "missing primary key `{}` in record",
            primary_key.name
        ))
    })?;
    Ok(Condition::compare(
        Expr::column(ColumnRef::new(table.name(), primary_key.name).typed(primary_key.db_type)),
        ComparisonOp::Eq,
        Expr::value(value),
    ))
}

fn expect_single_record(operation: &str, mut records: Vec<DbRecord>) -> Result<DbRecord> {
    match records.len() {
        1 => Ok(records.remove(0)),
        0 => Err(Error::message(format!(
            "{operation} did not return a record"
        ))),
        _ => Err(Error::message(format!(
            "{operation} returned more than one record unexpectedly"
        ))),
    }
}

fn merge_condition(existing: Option<Condition>, next: Condition) -> Option<Condition> {
    Some(match existing {
        Some(existing) => Condition::and([existing, next]),
        None => next,
    })
}

fn merge_optional_condition(
    existing: Option<Condition>,
    next: Option<Condition>,
) -> Option<Condition> {
    match next {
        Some(next) => merge_condition(existing, next),
        None => existing,
    }
}

fn ensure_insert_row(
    rows: &mut Vec<Vec<(super::ast::ColumnRef, Expr)>>,
) -> &mut Vec<(super::ast::ColumnRef, Expr)> {
    if rows.is_empty() {
        rows.push(Vec::new());
    }
    let index = rows.len() - 1;
    &mut rows[index]
}

fn push_insert_expr_value(insert: &mut InsertNode, value: (super::ast::ColumnRef, Expr)) {
    match &mut insert.source {
        InsertSource::Values(rows) => {
            if rows.is_empty() {
                rows.push(Vec::new());
            }
            let index = rows.len() - 1;
            rows[index].push(value);
        }
        InsertSource::Select(_) => {
            insert.source = InsertSource::Values(vec![vec![value]]);
        }
    }
}

fn push_insert_expr_row(insert: &mut InsertNode, row: Vec<(super::ast::ColumnRef, Expr)>) {
    match &mut insert.source {
        InsertSource::Values(rows) => rows.push(row),
        InsertSource::Select(_) => {
            insert.source = InsertSource::Values(vec![row]);
        }
    }
}

fn current_conflict_action(existing: Option<OnConflictNode>) -> OnConflictAction {
    existing
        .map(|node| node.action)
        .unwrap_or(OnConflictAction::DoNothing)
}

fn upsert_node(insert: &mut InsertNode) -> &mut OnConflictNode {
    insert.on_conflict.get_or_insert(OnConflictNode {
        target: None,
        action: OnConflictAction::DoNothing,
    })
}

fn upsert_node_model(on_conflict: &mut Option<OnConflictNode>) -> &mut OnConflictNode {
    on_conflict.get_or_insert(OnConflictNode {
        target: None,
        action: OnConflictAction::DoNothing,
    })
}

async fn decode_wrapped_projection<E, T>(
    executor: &E,
    ast: QueryAst,
    projection: AggregateProjection<T>,
) -> Result<T>
where
    E: QueryExecutor,
    T: FromDbValue,
{
    let compiled = PostgresCompiler::compile(&ast)?;
    let record = executor
        .query_records(&compiled)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| Error::message("aggregate query returned no rows"))?;
    projection.decode(&record)
}

async fn explain_query<E>(
    executor: &E,
    compiled: &super::compiler::CompiledSql,
    analyze: bool,
    options: QueryExecutionOptions,
) -> Result<Vec<String>>
where
    E: QueryExecutor,
{
    let sql = if analyze {
        format!("EXPLAIN ANALYZE {}", compiled.sql)
    } else {
        format!("EXPLAIN {}", compiled.sql)
    };
    let records = executor
        .raw_query_with(&sql, &compiled.bindings, options)
        .await?;
    records
        .iter()
        .map(|record| record.decode::<String>("QUERY PLAN"))
        .collect()
}

struct ModelStreamState<'a, M: Model> {
    executor: &'a dyn QueryExecutor,
    root_stream: DbRecordStream<'a>,
    table: &'static TableMeta<M>,
    relations: Vec<AnyRelation<M>>,
    relation_aggregates: Vec<AnyRelationAggregate<M>>,
    stream_batch_size: usize,
    buffered: VecDeque<Result<M>>,
    pending_error: Option<Error>,
    finished: bool,
    options: QueryExecutionOptions,
}

fn model_query_stream<'a, M>(state: ModelStreamState<'a, M>) -> BoxStream<'a, Result<M>>
where
    M: Model,
{
    stream::unfold(state, |mut state| async move {
        loop {
            if let Some(item) = state.buffered.pop_front() {
                return Some((item, state));
            }

            if let Some(error) = state.pending_error.take() {
                return Some((Err(error), state));
            }

            if state.finished {
                return None;
            }

            match fill_model_stream_buffer(&mut state).await {
                Ok(()) => {}
                Err(error) => {
                    state.finished = true;
                    return Some((Err(error), state));
                }
            }
        }
    })
    .boxed()
}

async fn fill_model_stream_buffer<M>(state: &mut ModelStreamState<'_, M>) -> Result<()>
where
    M: Model,
{
    let mut records = Vec::new();

    while records.len() < state.stream_batch_size {
        match state.root_stream.next().await {
            Some(Ok(record)) => records.push(record),
            Some(Err(error)) => {
                let error = wrap_model_stream_error(&state.options, "read root rows", error);
                if records.is_empty() {
                    return Err(error);
                }
                state.pending_error = Some(error);
                state.finished = true;
                break;
            }
            None => {
                state.finished = true;
                break;
            }
        }
    }

    if records.is_empty() {
        return Ok(());
    }

    let models = hydrate_model_batch(
        state.executor,
        state.table,
        &state.relations,
        &state.relation_aggregates,
        &records,
        &state.options,
    )
    .await?;

    state.buffered.extend(models.into_iter().map(Ok));
    Ok(())
}

async fn hydrate_model_batch<M>(
    executor: &dyn QueryExecutor,
    table: &'static TableMeta<M>,
    relations: &[AnyRelation<M>],
    relation_aggregates: &[AnyRelationAggregate<M>],
    records: &[DbRecord],
    options: &QueryExecutionOptions,
) -> Result<Vec<M>>
where
    M: Model,
{
    let mut models = records
        .iter()
        .map(|record| table.hydrate_record(record))
        .collect::<Result<Vec<_>>>()
        .map_err(|error| wrap_model_query_batch_error(options, "hydrate root rows", error))?;

    for relation in relations {
        relation
            .load(executor, &mut models)
            .await
            .map_err(|error| {
                wrap_model_query_batch_error(options, "load eager relations", error)
            })?;
    }

    for aggregate in relation_aggregates {
        aggregate
            .load(executor, &mut models)
            .await
            .map_err(|error| {
                wrap_model_query_batch_error(options, "load relation aggregates", error)
            })?;
    }

    Ok(models)
}

fn wrap_model_query_batch_error(
    options: &QueryExecutionOptions,
    action: &str,
    error: Error,
) -> Error {
    let label = options
        .label
        .as_deref()
        .map(|label| format!(" in `{label}`"))
        .unwrap_or_default();
    Error::message(format!("model query failed to {action}{label}: {error}"))
}

fn wrap_model_stream_error(options: &QueryExecutionOptions, action: &str, error: Error) -> Error {
    let label = options
        .label
        .as_deref()
        .map(|label| format!(" in `{label}`"))
        .unwrap_or_default();
    Error::message(format!(
        "model query stream failed to {action}{label}: {error}"
    ))
}

#[cfg(test)]
mod tests {
    use super::{InsertSource, Query, QueryBody};

    #[test]
    fn insert_builder_switches_from_select_source_to_values_without_panicking() {
        let query = Query::insert_select_into("audit_logs", Query::table("users").select(["id"]))
            .value("id", 1_i64)
            .value("label", "one");
        let ast = query.ast();

        let QueryBody::Insert(insert) = &ast.body else {
            panic!("expected insert query body");
        };

        match &insert.source {
            InsertSource::Values(rows) => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].len(), 2);
            }
            InsertSource::Select(_) => panic!("insert source should have normalized to values"),
        }
    }
}
