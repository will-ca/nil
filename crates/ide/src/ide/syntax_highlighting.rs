//! This is actually so-called "semantic highlighting".
//! Ref: <https://github.com/rust-lang/rust-analyzer/blob/a670ff888437f4b6a3d24cc2996e9f969a87cbae/crates/ide/src/syntax_highlighting/tags.rs>
use crate::def::{AstPtr, Expr, NameKind, ResolveResult};
use crate::{DefDatabase, FileId};
use builtin::{BuiltinKind, ALL_BUILTINS};
use syntax::{SyntaxKind, SyntaxToken, TextRange, T};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlRange {
    pub range: TextRange,
    pub tag: HlTag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HlTag {
    NameDef(NameKind),
    NameRef(NameKind),
    UnresolvedRef,

    AttrField,
    Builtin(BuiltinKind),
    Comment,
    FloatLiteral,
    IntLiteral,
    Keyword(HlKeyword),
    Operator(HlOperator),
    Path,
    Punct(HlPunct),
    StringEscape,
    StringLiteral,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HlKeyword {
    Conditional,
    Operator,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HlOperator {
    Logical,
    Comparison,
    Arithmetic,
    Aggregation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HlPunct {
    Brace,
    Bracket,
    Paren,
    Dot,
    Question,
    Comma,
    Semicolon,
    Equal,
    Colon,
    At,
    Ellipsis,
}

pub(crate) fn highlight(
    db: &dyn DefDatabase,
    file: FileId,
    range: Option<TextRange>,
) -> Vec<HlRange> {
    let root_node = db.parse(file).syntax_node();
    let source_map = db.source_map(file);
    let nameres = db.name_resolution(file);
    let module = db.module(file);

    let highlight_token = |tok: &SyntaxToken| -> Option<HlRange> {
        let tag = match tok.kind() {
            SyntaxKind::SPACE => return None,
            SyntaxKind::COMMENT => HlTag::Comment,
            SyntaxKind::PATH | SyntaxKind::SEARCH_PATH => HlTag::Path,
            SyntaxKind::FLOAT => HlTag::FloatLiteral,
            SyntaxKind::INT => HlTag::IntLiteral,
            T!["''"] | T!['"'] | SyntaxKind::URI => HlTag::StringLiteral,
            SyntaxKind::STRING_ESCAPE => HlTag::StringEscape,
            // Don't color the string content. They are subjects to injection of other languages.
            SyntaxKind::STRING_FRAGMENT => return None,

            T![&&] | T![||] | T![->] | T![!] => HlTag::Operator(HlOperator::Logical),
            T![==] | T![!=] | T![<] | T![>] | T![<=] | T![>=] => {
                HlTag::Operator(HlOperator::Comparison)
            }
            T![+] | T![-] | T![*] | T![/] => HlTag::Operator(HlOperator::Arithmetic),
            T![++] | T!["//"] => HlTag::Operator(HlOperator::Aggregation),
            T!['{'] | T!['}'] | T!["${"] => HlTag::Punct(HlPunct::Brace),
            T!['['] | T![']'] => HlTag::Punct(HlPunct::Bracket),
            T!['('] | T![')'] => HlTag::Punct(HlPunct::Paren),
            T![.] => HlTag::Punct(HlPunct::Dot),
            T![?] => HlTag::Punct(HlPunct::Question),
            T![,] => HlTag::Punct(HlPunct::Comma),
            T![;] => HlTag::Punct(HlPunct::Semicolon),
            T![=] => HlTag::Punct(HlPunct::Equal),
            T![:] => HlTag::Punct(HlPunct::Colon),
            T![@] => HlTag::Punct(HlPunct::At),
            T![...] => HlTag::Punct(HlPunct::Ellipsis),

            T![if] | T![then] | T![else] => HlTag::Keyword(HlKeyword::Conditional),
            T![or] => HlTag::Keyword(HlKeyword::Operator),
            T![assert] | T![in] | T![inherit] | T![let] | T![rec] | T![with] => {
                HlTag::Keyword(HlKeyword::Other)
            }

            SyntaxKind::IDENT => match tok.parent() {
                Some(node) if node.kind() == SyntaxKind::REF => {
                    let expr = source_map.expr_for_node(AstPtr::new(&node))?;
                    match nameres.get(expr) {
                        None => HlTag::UnresolvedRef,
                        Some(ResolveResult::Definition(def)) => HlTag::NameRef(module[*def].kind),
                        Some(ResolveResult::WithExprs(_)) => HlTag::NameRef(NameKind::PlainAttrset),
                        Some(ResolveResult::Builtin(name)) => {
                            HlTag::Builtin(ALL_BUILTINS[*name].kind)
                        }
                    }
                }
                Some(node) if node.kind() == SyntaxKind::NAME => {
                    let ptr = AstPtr::new(&node);
                    match source_map.name_for_node(ptr.clone()) {
                        Some(name) => HlTag::NameDef(module[name].kind),
                        None => {
                            match source_map.expr_for_node(ptr) {
                                // `Attr`s are converted into string literals.
                                Some(expr) if matches!(&module[expr], Expr::Literal(_)) => {
                                    HlTag::AttrField
                                }
                                _ => return None,
                            }
                        }
                    }
                }
                _ => return None,
            },
            _ => return None,
        };

        Some(HlRange {
            range: tok.text_range(),
            tag,
        })
    };

    let (first_tok, end_pos) = match range {
        None => (root_node.first_token(), u32::MAX.into()),
        Some(range) => (
            root_node.token_at_offset(range.start()).right_biased(),
            range.end(),
        ),
    };

    std::iter::successors(first_tok, |tok| tok.next_token())
        .take_while(|tok| tok.text_range().start() < end_pos)
        .filter_map(|tok| highlight_token(&tok))
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::tests::TestDB;
    use crate::{DefDatabase, FilePos};
    use expect_test::{expect, Expect};

    #[track_caller]
    fn check(fixture: &str, expect: Expect) {
        let (db, f) = TestDB::from_fixture(fixture).unwrap();
        let FilePos { file_id, pos } = f[0];
        assert_eq!(db.parse(file_id).errors(), &[]);
        let hls = super::highlight(&db, file_id, None);
        // No overlapping.
        for w in hls.windows(2) {
            assert!(w[0].range.end() <= w[1].range.start());
        }
        let hlrange = hls
            .iter()
            .find(|hlrange| hlrange.range.contains(pos))
            .expect("No highlight found");
        let got = format!("{:?}", hlrange.tag);
        expect.assert_eq(&got);
    }

    #[test]
    fn keyword() {
        check("$0if 1 then 2 else 3", expect!["Keyword(Conditional)"]);
        check("$0let in 1", expect!["Keyword(Other)"]);
        check("a.b $0or c", expect!["Keyword(Operator)"]);

        // Contextual keywords.
        check("let or = 1; in a $0or", expect!["NameRef(LetIn)"]);
        check("{ $0or = 1; }", expect!["NameDef(PlainAttrset)"]);
    }

    #[test]
    fn operator() {
        check("1 $0+ 1", expect!["Operator(Arithmetic)"]);
        check("1 $0< 1", expect!["Operator(Comparison)"]);
        check("true $0-> false", expect!["Operator(Logical)"]);
        check("[] $0++ []", expect!["Operator(Aggregation)"]);
    }

    #[test]
    fn comment() {
        check("1/*$0a*/", expect!["Comment"]);
        check("1#$0a", expect!["Comment"]);
    }

    #[test]
    fn path() {
        check("$0./.", expect!["Path"]);
    }

    #[test]
    fn literal() {
        check("$042", expect!["IntLiteral"]);
        check("$01.0", expect!["FloatLiteral"]);
        check("$0a:b", expect!["StringLiteral"]);
        check(r#"$0"string""#, expect!["StringLiteral"]);
        check(r#""st$0\nring""#, expect!["StringEscape"]);
    }

    #[test]
    fn builtins() {
        check("$0true", expect!["Builtin(Const)"]);
        check("$0builtins", expect!["Builtin(Attrset)"]);
        check("$0map", expect!["Builtin(Function)"]);
    }

    #[test]
    fn name() {
        check("let $0a = 1; in a", expect!["NameDef(LetIn)"]);
        check("let a = 1; in $0a", expect!["NameRef(LetIn)"]);

        check("$0a: a", expect!["NameDef(Param)"]);
        check("a: $0a", expect!["NameRef(Param)"]);
        check("{ $0a }@b: a", expect!["NameDef(PatField)"]);
        check("{ a }@b: $0a", expect!["NameRef(PatField)"]);
        check("{ a }@$0b: b", expect!["NameDef(Param)"]);
        check("{ a }@b: $0b", expect!["NameRef(Param)"]);

        check("rec { $0a = 1; b = a; }", expect!["NameDef(RecAttrset)"]);
        check("rec { a = 1; b = $0a; }", expect!["NameRef(RecAttrset)"]);

        check(
            "let a = 1; in { inherit $0a; }",
            expect!["NameDef(PlainAttrset)"],
        );
        check(
            "rec { a = 1; b = { inherit $0a; }; }",
            expect!["NameDef(PlainAttrset)"],
        );

        check("let true = 1; in $0true", expect!["NameRef(LetIn)"]);

        check("with {}; $0a", expect!["NameRef(PlainAttrset)"]);

        check("$0not_found", expect!["UnresolvedRef"]);
    }

    #[test]
    fn attr() {
        check("{}.$0a", expect!["AttrField"]);
        check("{} ? $0a", expect!["AttrField"]);
    }
}
