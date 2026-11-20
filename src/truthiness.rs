// Copyright 2024 KipData/KiteSQL
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! MySQL truthiness: a number is a condition, true when it is not 0, so `IF(ISNULL(x), 0, 1)`,
//! `flag AND IF(...)`, `NOT n` and `WHERE ISNULL(x)` all work. The engine wants a boolean in
//! these places, so an operand that is not already a boolean expression becomes `(operand <> 0)`.
//! NULL stays NULL, and boolean values compare with 0 as MySQL's TRUE/FALSE (1/0) do.

use sqlparser::ast::{
    BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, ObjectNamePart, Query,
    SetExpr, Statement, UnaryOperator, Value, VisitMut, VisitorMut,
};
use std::ops::ControlFlow;

/// Rewrites every condition position in `stmt`.
pub(crate) fn rewrite(stmt: &mut Statement) {
    let _ = stmt.visit(&mut Conditions);
}

struct Conditions;

impl VisitorMut for Conditions {
    type Break = ();

    fn post_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<()> {
        match expr {
            Expr::BinaryOp {
                left,
                op: BinaryOperator::And | BinaryOperator::Or | BinaryOperator::Xor,
                right,
            } => {
                condition(left);
                condition(right);
            }
            Expr::UnaryOp {
                op: UnaryOperator::Not,
                expr,
            } => condition(expr),
            Expr::Case {
                operand: None,
                conditions,
                ..
            } => {
                for when in conditions {
                    condition(&mut when.condition);
                }
            }
            Expr::Function(function)
                if function.name.0.len() == 1
                    && matches!(&function.name.0[0], ObjectNamePart::Identifier(id) if id.value.eq_ignore_ascii_case("if")) =>
            {
                if let FunctionArguments::List(list) = &mut function.args {
                    if list.args.len() == 3 {
                        if let FunctionArg::Unnamed(FunctionArgExpr::Expr(first)) =
                            &mut list.args[0]
                        {
                            condition(first);
                        }
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, query: &mut Query) -> ControlFlow<()> {
        set_expr(&mut query.body);
        ControlFlow::Continue(())
    }

    fn post_visit_statement(&mut self, statement: &mut Statement) -> ControlFlow<()> {
        match statement {
            Statement::Update(update) => optional(&mut update.selection),
            Statement::Delete(delete) => optional(&mut delete.selection),
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

fn set_expr(body: &mut SetExpr) {
    match body {
        SetExpr::Select(select) => {
            optional(&mut select.selection);
            optional(&mut select.having);
        }
        SetExpr::SetOperation { left, right, .. } => {
            set_expr(left);
            set_expr(right);
        }
        _ => {}
    }
}

fn optional(expr: &mut Option<Expr>) {
    if let Some(expr) = expr {
        condition(expr);
    }
}

/// Makes `expr` a boolean: unchanged when it already is one, `(expr <> 0)` otherwise.
fn condition(expr: &mut Expr) {
    if is_boolean(expr) {
        return;
    }
    let operand = std::mem::replace(expr, Expr::value(Value::Null));
    *expr = Expr::Nested(Box::new(Expr::BinaryOp {
        left: Box::new(operand),
        op: BinaryOperator::NotEq,
        right: Box::new(Expr::value(Value::Number("0".into(), false))),
    }));
}

/// Whether `expr` is a boolean by its form: a comparison, a logical operator, a predicate, a
/// boolean or NULL literal, or a subquery (left to the engine).
fn is_boolean(expr: &Expr) -> bool {
    match expr {
        Expr::Nested(inner) => is_boolean(inner),
        Expr::BinaryOp { op, .. } => matches!(
            op,
            BinaryOperator::Eq
                | BinaryOperator::NotEq
                | BinaryOperator::Lt
                | BinaryOperator::LtEq
                | BinaryOperator::Gt
                | BinaryOperator::GtEq
                | BinaryOperator::Spaceship
                | BinaryOperator::And
                | BinaryOperator::Or
                | BinaryOperator::Xor
        ),
        Expr::UnaryOp { op, .. } => matches!(op, UnaryOperator::Not),
        Expr::Value(value) => matches!(value.value, Value::Boolean(_) | Value::Null),
        Expr::IsNull(_)
        | Expr::IsNotNull(_)
        | Expr::IsTrue(_)
        | Expr::IsNotTrue(_)
        | Expr::IsFalse(_)
        | Expr::IsNotFalse(_)
        | Expr::IsUnknown(_)
        | Expr::IsNotUnknown(_)
        | Expr::IsDistinctFrom(..)
        | Expr::IsNotDistinctFrom(..)
        | Expr::Like { .. }
        | Expr::ILike { .. }
        | Expr::SimilarTo { .. }
        | Expr::RLike { .. }
        | Expr::InList { .. }
        | Expr::InSubquery { .. }
        | Expr::InUnnest { .. }
        | Expr::Between { .. }
        | Expr::Exists { .. }
        | Expr::AnyOp { .. }
        | Expr::AllOp { .. }
        | Expr::Subquery(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlparser::dialect::MySqlDialect;
    use sqlparser::parser::Parser;

    fn rewritten(sql: &str) -> String {
        let mut stmt = Parser::parse_sql(&MySqlDialect {}, sql).unwrap().remove(0);
        rewrite(&mut stmt);
        stmt.to_string()
    }

    #[test]
    fn numeric_conditions_compare_with_zero() {
        assert_eq!(
            rewritten("SELECT IF(ISNULL(a), 0, 1) AND b FROM t"),
            "SELECT (IF((ISNULL(a) <> 0), 0, 1) <> 0) AND (b <> 0) FROM t"
        );
        assert_eq!(
            rewritten("SELECT NOT n, CASE WHEN n THEN 1 END FROM t WHERE ISNULL(a) HAVING COUNT(*)"),
            "SELECT NOT (n <> 0), CASE WHEN (n <> 0) THEN 1 END FROM t WHERE (ISNULL(a) <> 0) HAVING (COUNT(*) <> 0)"
        );
        assert_eq!(
            rewritten("UPDATE t SET a = 1 WHERE n"),
            "UPDATE t SET a = 1 WHERE (n <> 0)"
        );
        assert_eq!(
            rewritten("DELETE FROM t WHERE n"),
            "DELETE FROM t WHERE (n <> 0)"
        );
    }

    #[test]
    fn boolean_conditions_are_kept() {
        for sql in [
            "SELECT a FROM t WHERE a = 1 AND (b IS NULL OR c LIKE 'x%') AND NOT d IN (1, 2)",
            "SELECT IF(a > 1, 0, 1), CASE WHEN a BETWEEN 1 AND 2 THEN 1 END FROM t WHERE true",
            "SELECT CASE a WHEN 1 THEN 2 END FROM t WHERE EXISTS (SELECT 1)",
            "SELECT a FROM t WHERE NULL",
        ] {
            assert_eq!(rewritten(sql), sql);
        }
    }
}
