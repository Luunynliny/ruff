use crate::ast_node_ref::AstNodeRef;
use crate::db::Db;
use crate::semantic_index::place::{FileScopeId, ScopeId};
use ruff_db::files::File;
use ruff_db::parsed::ParsedModuleRef;
use ruff_python_ast as ast;
use salsa;

/// Whether or not this expression should be inferred as a normal expression or
/// a type expression. For example, in `self.x: <annotation> = <value>`, the
/// `<annotation>` is inferred as a type expression, while `<value>` is inferred
/// as a normal expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ExpressionKind {
    Normal,
    TypeExpression,
}

/// An independently type-inferable expression.
///
/// Includes constraint expressions (e.g. if tests) and the RHS of an unpacking assignment.
///
/// ## Module-local type
/// This type should not be used as part of any cross-module API because
/// it holds a reference to the AST node. Range-offset changes
/// then propagate through all usages, and deserialization requires
/// reparsing the entire module.
///
/// E.g. don't use this type in:
///
/// * a return type of a cross-module query
/// * a field of a type that is a return type of a cross-module query
/// * an argument of a cross-module query
#[salsa::tracked(debug)]
pub(crate) struct Expression<'db> {
    /// The file in which the expression occurs.
    pub(crate) file: File,

    /// The scope in which the expression occurs.
    pub(crate) file_scope: FileScopeId,

    /// The expression node.
    #[no_eq]
    #[tracked]
    #[returns(ref)]
    pub(crate) _node_ref: AstNodeRef<ast::Expr>,

    /// An assignment statement, if this expression is immediately used as the rhs of that
    /// assignment.
    ///
    /// (Note that this is the _immediately_ containing assignment — if a complex expression is
    /// assigned to some target, only the outermost expression node has this set. The inner
    /// expressions are used to build up the assignment result, and are not "immediately assigned"
    /// to the target, and so have `None` for this field.)
    #[no_eq]
    #[tracked]
    pub(crate) assigned_to: Option<AstNodeRef<ast::StmtAssign>>,

    /// Should this expression be inferred as a normal expression or a type expression?
    pub(crate) kind: ExpressionKind,
}

// The Salsa heap is tracked separately.
impl get_size2::GetSize for Expression<'_> {}

impl<'db> Expression<'db> {
    pub(crate) fn node_ref<'ast>(
        self,
        db: &'db dyn Db,
        parsed: &'ast ParsedModuleRef,
    ) -> &'ast ast::Expr {
        self._node_ref(db).node(parsed)
    }

    pub(crate) fn scope(self, db: &'db dyn Db) -> ScopeId<'db> {
        self.file_scope(db).to_scope_id(db, self.file(db))
    }
}
