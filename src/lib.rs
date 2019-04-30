#![cfg_attr(use_proc_macro_diagnostic, feature(proc_macro_diagnostic))]
extern crate proc_macro;

use proc_macro2::TokenStream;
use quote::{quote, TokenStreamExt, ToTokens};
use syn::{
    ArgCaptured, Expr, FnArg, Ident, ItemFn,
    parse_macro_input, parse_str, Pat,
    Stmt,
};

use error::error_statement;
use parse::{Modifiers, RsTestAttribute};

mod parse;
mod error;


trait Tokenize {
    fn into_tokens(self) -> TokenStream;
}

impl<T: ToTokens> Tokenize for T {
    fn into_tokens(self) -> TokenStream {
        quote! { #self }
    }
}

fn default_fixture_resolve(ident: &Ident) -> parse::CaseArg {
    let e = parse_str::<Expr>(&format!("{}()", ident.to_string())).unwrap();
    parse::CaseArg::from(e)
}

fn fn_arg_ident(arg: &FnArg) -> Option<&Ident> {
    match arg {
        FnArg::Captured(ArgCaptured { pat: Pat::Ident(ident), .. }) => Some(&ident.ident),
        _ => None
    }
}

fn arg_2_fixture(ident: &Ident, resolver: &Resolver) -> TokenStream {
    let fixture = resolver
        .resolve(ident)
        .map(|e| e.clone())
        .unwrap_or_else(|| default_fixture_resolve(ident));
    quote! {
        let #ident = #fixture;
    }
}

fn arg_2_fixture_dump_str(ident: &Ident) -> String {
    format!(r#"println!("{name} = {{:?}}", {name});"#, name = ident)
}

fn arg_2_fixture_dump(ident: &Ident, modifiers: &Modifiers) -> Option<Stmt> {
    if modifiers.trace_me(ident) {
        parse_str(&arg_2_fixture_dump_str(ident)).ok()
    } else {
        None
    }
}

#[derive(Default)]
/// `Resolver` can `resolve` an ident to a `CaseArg`. Pass it to `render_fn_test`
/// function to inject the case arguments resolution.
struct Resolver<'a> (std::collections::HashMap<String, &'a parse::CaseArg>);

impl<'a> Resolver<'a> {
    fn new(args: &Vec<Ident>, case: &'a parse::TestCase) -> Self {
        Resolver(
            args.iter()
                .zip(case.args.iter())
                .map(|(ref name, case_arg)| (name.to_string(), case_arg))
                .collect()
        )
    }

    fn resolve(&self, ident: &Ident) -> Option<&parse::CaseArg> {
        self.0.get(&ident.to_string()).map(|&a| a)
    }
}

fn fn_args(item_fn: &ItemFn) -> impl Iterator<Item=&FnArg> {
    item_fn.decl.inputs.iter()
}

const TRACE_VARIABLE_ATTR: &'static str = "trace";
const NOTRACE_VARIABLE_ATTR: &'static str = "notrace";

impl Modifiers {
    fn trace_me(&self, ident: &Ident) -> bool {
        if self.should_trace() {
            self.modifiers
                .iter()
                .filter(|&m|
                    Modifiers::is_notrace(ident, m)
                ).next().is_none()
        } else { false }
    }

    fn is_notrace(ident: &Ident, m: &RsTestAttribute) -> bool {
        match m {
            RsTestAttribute::Tagged(i, args) if i == NOTRACE_VARIABLE_ATTR =>
                args.iter().find(|&a| a == ident).is_some(),
            _ => false
        }
    }

    fn should_trace(&self) -> bool {
        self.modifiers
            .iter()
            .filter(|&m|
                Modifiers::is_trace(m)
            ).next().is_some()
    }

    fn is_trace(m: &RsTestAttribute) -> bool {
        match m {
            RsTestAttribute::Attr(i) if i == TRACE_VARIABLE_ATTR => true,
            _ => false
        }
    }
}

trait Iterable<I, IT: Iterator<Item=I>, OUT: Iterator<Item=I>> {
    fn iterable(self) -> Option<OUT>;
}

impl<I, IT: Iterator<Item=I>> Iterable<I, IT, std::iter::Peekable<IT>> for IT {
    fn iterable(self) -> Option<std::iter::Peekable<IT>> {
        let mut peekable = self.peekable();
        if peekable.peek().is_some() {
            Some(peekable)
        } else {
            None
        }
    }
}

fn trace_arguments(args: &Vec<Ident>, modifiers: &Modifiers) -> Option<proc_macro2::TokenStream> {
    args.iter()
        .filter_map(move |arg| arg_2_fixture_dump(arg, modifiers))
        .iterable()
        .map(
            |it|
                quote! {
                    println!("{:-^40}", " TEST ARGUMENTS ");
                    #(#it)*
                }
        )
}

fn render_fn_test<'a>(name: Ident, testfn: &ItemFn,
                      resolver: &'a Resolver, modifiers: &'a Modifiers, inline_impl: bool)
                      -> TokenStream {
    let testfn_name = &testfn.ident;
    let args = fn_args_idents(&testfn);
    let attrs = &testfn.attrs;
    let output = &testfn.decl.output;
    let test_impl = if inline_impl { Some(testfn) } else { None };
    let fixtures = args.iter()
        .map(move |arg| arg_2_fixture(arg, resolver));
    let trace_args = trace_arguments(&args, modifiers);
    quote! {
        #[test]
        #(#attrs)*
        fn #name() #output {
            #test_impl
            #(#fixtures)*
            #trace_args
            println!("{:-^40}", " TEST START ");
            #testfn_name(#(#args),*)
        }
    }
}

fn render_fixture<'a>(fixture: ItemFn, resolver: Resolver,
                      _modifiers: Modifiers)
                      -> TokenStream {
    let name = &fixture.ident;
    let orig_args = &fixture.decl.inputs;
    let vargs = fn_args_idents(&fixture);
    let args = &vargs;
    let attrs = &fixture.attrs;
    let output = &fixture.decl.output;
    let visibility = &fixture.vis;
    let (store_type, generics) = convert_output_type(output);
    let vresolve_args = args.iter()
        .map(move |arg| arg_2_fixture(arg, &resolver))
        .collect::<Vec<_>>();
    let resolve_args = &vresolve_args;
    quote! {
        #[allow(non_camel_case_types)]
        #visibility struct #name {
            data: Option<#store_type>
        }

        impl #name {
            pub fn new(#orig_args) -> Self {
                #fixture
                Self { data: Some(#name(#(#args),*)) }
            }

            pub fn take(&mut self) #output {
                self.data.take().unwrap()
            }
        }

        impl std::default::Default for #name {
            fn default() -> Self {
                #(#resolve_args)*
                Self::new(#(#args),*)
            }
        }

        #(#attrs)*
        #visibility fn #name() #output {
            #fixture
            #(#resolve_args)*
            #name(#(#args),*)
        }
    }
}

fn convert_output_type(output: &syn::ReturnType) -> (&dyn quote::ToTokens, impl quote::ToTokens) {
    (match output {
        syn::ReturnType::Default => output,
        syn::ReturnType::Type(_, inner) =>
            match inner {
                _ => inner
            }
    }, quote! {})
}

fn fn_args_idents(test: &ItemFn) -> Vec<Ident> {
    fn_args(&test)
        .filter_map(fn_arg_ident)
        .cloned()
        .collect::<Vec<_>>()
}

#[proc_macro_attribute]
pub fn fixture(args: proc_macro::TokenStream,
              input: proc_macro::TokenStream)
              -> proc_macro::TokenStream {
    let fixture = parse_macro_input!(input as ItemFn);
    let modifiers = parse_macro_input!(args as Modifiers);
    render_fixture(fixture, Resolver::default(), modifiers).into()
}

#[proc_macro_attribute]
pub fn rstest(args: proc_macro::TokenStream,
              input: proc_macro::TokenStream)
              -> proc_macro::TokenStream {
    let test = parse_macro_input!(input as ItemFn);
    let modifiers = parse_macro_input!(args as Modifiers);
    let name = &test.ident;
    let resolver = Resolver::default();
    render_fn_test(name.clone(), &test, &resolver, &modifiers, true)
        .into()
}

fn fn_args_has_ident(fn_decl: &ItemFn, ident: &Ident) -> bool {
    fn_args(fn_decl)
        .filter_map(fn_arg_ident)
        .find(|&id| id == ident)
        .is_some()
}

fn errors_in_parametrize(test: &ItemFn, params: &parse::ParametrizeData) -> Option<TokenStream> {
    let invalid_args = params.args.iter()
        .filter(|&p| !fn_args_has_ident(test, p));

    let mut tokens = TokenStream::default();
    for missed in invalid_args {
        let span = missed.span().into();
        let message = format!("Missed argument: '{}' should be a test function argument.", missed);
        tokens.extend(error_statement(&message, span, span));
    }

    if !tokens.is_empty() {
        Some(tokens)
    } else {
        None
    }
}

fn add_parametrize_cases(test: ItemFn, params: parse::ParametrizeInfo) -> TokenStream {
    let fname = &test.ident;
    let parse::ParametrizeInfo { data: params, modifier } = params;

    let mut cases = TokenStream::new();
    for (n, case) in params.cases.iter().enumerate() {
        cases.append_all(
            if case.args.len() != params.args.len() {
                error_statement("Wrong case signature: should match the given parameters list.",
                                case.span_start(), case.span_end())
            } else {
                let resolver = Resolver::new(&params.args, &case);
                let name = Ident::new(&format_case_name(&params, n), fname.span());
                render_fn_test(name, &test, &resolver, &modifier, false)
            }
        )
    };
    quote! {
        #[cfg(test)]
        #test

        #[cfg(test)]
        mod #fname {
            use super::*;

            #cases
        }
    }
}

fn format_case_name(params: &parse::ParametrizeData, index: usize) -> String {
    let len_max = format!("{}", params.cases.len()).len();
    let description = params.cases[index]
        .description.as_ref()
        .map(|d| format!("_{}", d))
        .unwrap_or_default();
    format!("case_{:0len$}{d}", index + 1, len = len_max as usize, d = description)
}

#[proc_macro_attribute]
pub fn rstest_parametrize(args: proc_macro::TokenStream, input: proc_macro::TokenStream)
                          -> proc_macro::TokenStream
{
    let params = parse_macro_input!(args as parse::ParametrizeInfo);
    let test = parse_macro_input!(input as ItemFn);

    if let Some(tokens) = errors_in_parametrize(&test, &params.data) {
        tokens
    } else {
        add_parametrize_cases(test, params)
    }.into()
}

#[cfg(test)]
mod render {
    use pretty_assertions::assert_eq;
    use syn::{ItemFn, punctuated};
    use syn::export::Debug;
    use syn::parse2;

    use crate::parse::*;

    use super::*;

    fn fn_args(item: &ItemFn) -> punctuated::Iter<'_, FnArg> {
        item.decl.inputs.iter()

    }

    fn first_arg_ident(ast: &ItemFn) -> &Ident {
        let arg = fn_args(&ast).next().unwrap();
        fn_arg_ident(arg).unwrap()
    }

    fn assert_syn_eq<P, S>(expected: S, ast: P) where
        S: AsRef<str>,
        P: syn::parse::Parse + Debug + Eq
    {
        assert_eq!(
            parse_str::<P>(expected.as_ref()).unwrap(),
            ast
        )
    }

    fn assert_statement_eq<T, S>(expected: S, tokens: T) where
        T: Into<TokenStream>,
        S: AsRef<str>
    {
        assert_syn_eq::<Stmt, _>(expected, parse2::<Stmt>(tokens.into()).unwrap())
    }

    #[test]
    fn extract_fixture_call_arg() {
        let ast = parse_str("fn foo(mut fix: String) {}").unwrap();
        let arg = first_arg_ident(&ast);
        let resolver = Resolver::default();

        let line = arg_2_fixture(arg, &resolver);

        assert_statement_eq("let fix = fix();", line);
    }

    #[test]
    fn extract_fixture_should_not_add_mut() {
        let ast = parse_str("fn foo(mut fix: String) {}").unwrap();
        let arg = first_arg_ident(&ast);
        let resolver = Resolver::default();

        let line = arg_2_fixture(arg, &resolver);

        assert_statement_eq("let fix = fix();", line);
    }

    fn case_arg<S: AsRef<str>>(s: S) -> CaseArg {
        parse_str::<Expr>(s.as_ref()).unwrap().into()
    }

    #[test]
    fn arg_2_fixture_str_should_use_passed_fixture_if_any() {
        let ast = parse_str("fn foo(mut fix: String) {}").unwrap();
        let arg = first_arg_ident(&ast);
        let call = case_arg("bar()");
        let mut resolver = Resolver::default();
        resolver.add("fix", &call);

        let line = arg_2_fixture(arg, &resolver);

        assert_statement_eq("let fix = bar();", line);
    }

    impl<'a> Resolver<'a> {
        fn add<S: AsRef<str>>(&mut self, ident: S, expr: &'a CaseArg) {
            self.0.insert(ident.as_ref().to_string(), expr);
        }
    }

    #[test]
    fn resolver_should_return_the_given_expression() {
        let ast = parse_str("fn function(mut foo: String) {}").unwrap();
        let arg = first_arg_ident(&ast);
        let expected = case_arg("bar()");
        let mut resolver = Resolver::default();

        resolver.add("foo", &expected);

        assert_eq!(&expected, resolver.resolve(&arg).unwrap())
    }

    #[test]
    fn resolver_should_return_none_for_unknown_argument() {
        let ast = parse_str("fn function(mut fix: String) {}").unwrap();
        let arg = first_arg_ident(&ast);
        let resolver = Resolver::default();

        assert!(resolver.resolve(&arg).is_none())
    }

    mod render_fn_test_should {
        use super::*;
        use proc_macro2::Span;
        use pretty_assertions::assert_eq;

        #[test]
        fn add_return_type_if_any() {
            let ast: ItemFn = parse_str("fn function(mut fix: String) -> Result<i32, String> { Ok(42) }").unwrap();

            let tokens = render_fn_test(Ident::new("new_name", Span::call_site()),
                                        &ast, &Resolver::default(), &Modifiers::default(), false);

            let result: ItemFn = parse2(tokens).unwrap();

            assert_eq!(result.ident.to_string(), "new_name");
            assert_eq!(result.decl.output, ast.decl.output);
        }
    }

    mod add_parametrize_cases {
        use std::borrow::Cow;

        use syn::{ItemFn, ItemMod, parse::{Parse, ParseStream, Result}};
        use syn::visit::Visit;

        use super::{*, assert_eq};

        struct ParametrizeOutput {
            orig: ItemFn,
            module: ItemMod,
        }

        impl Parse for ParametrizeOutput {
            fn parse(input: ParseStream) -> Result<Self> {
                Ok(Self {
                    orig: input.parse()?,
                    module: input.parse()?,
                })
            }
        }

        impl ParametrizeOutput {
            pub fn get_test_functions(&self) -> Vec<ItemFn> {
                let mut f = TestFunctions(vec![]);

                f.visit_item_mod(&self.module);
                f.0
            }
        }

        impl From<TokenStream> for ParametrizeOutput {
            fn from(tokens: TokenStream) -> Self {
                syn::parse2::<ParametrizeOutput>(tokens).unwrap()
            }
        }

        impl<'a> From<&'a ItemFn> for parse::ParametrizeData {
            fn from(item_fn: &'a ItemFn) -> Self {
                parse::ParametrizeData {
                    args: fn_args_idents(item_fn),
                    cases: vec![],
                }
            }
        }

        impl<'a> From<&'a ItemFn> for parse::ParametrizeInfo {
            fn from(item_fn: &'a ItemFn) -> Self {
                parse::ParametrizeInfo {
                    data: item_fn.into(),
                    modifier: Default::default(),
                }
            }
        }

        /// To extract all test functions
        struct TestFunctions(Vec<ItemFn>);

        impl TestFunctions {
            fn is_test_fn(item_fn: &ItemFn) -> bool {
                item_fn.attrs.iter().filter(|&a|
                    a.path == parse_str::<syn::Path>("test").unwrap())
                    .next().is_some()
            }
        }

        impl<'ast> Visit<'ast> for TestFunctions {
            fn visit_item_fn(&mut self, item_fn: &'ast ItemFn) {
                if Self::is_test_fn(item_fn) {
                    self.0.push(item_fn.clone())
                }
            }
        }


        #[test]
        fn should_create_a_module_named_as_test_function() {
            let item_fn = parse_str::<ItemFn>("fn should_be_the_module_name(mut fix: String) {}").unwrap();
            let info = (&item_fn).into();
            let tokens = add_parametrize_cases(item_fn.clone(), info);

            let output = ParametrizeOutput::from(tokens);

            assert_eq!(output.module.ident, "should_be_the_module_name");
        }

        #[test]
        fn should_copy_user_function() {
            let item_fn = parse_str::<ItemFn>(
                r#"fn should_be_the_module_name(mut fix: String) { println!("user code") }"#
            ).unwrap();
            let info = (&item_fn).into();
            let tokens = add_parametrize_cases(item_fn.clone(), info);

            let mut output = ParametrizeOutput::from(tokens);

            output.orig.attrs = vec![];
            assert_eq!(output.orig, item_fn);
        }

        #[test]
        fn should_mark_user_function_as_test() {
            let item_fn = parse_str::<ItemFn>(
                r#"fn should_be_the_module_name(mut fix: String) { println!("user code") }"#
            ).unwrap();
            let info = (&item_fn).into();
            let tokens = add_parametrize_cases(item_fn.clone(), info);

            let output = ParametrizeOutput::from(tokens);

            let expected = parse2::<ItemFn>(quote! {
                #[cfg(test)]
                fn some() {}
            }).unwrap().attrs;

            assert_eq!(expected, output.orig.attrs);
        }

        #[test]
        fn should_mark_module_as_test() {
            let item_fn = parse_str::<ItemFn>(
                r#"fn should_be_the_module_name(mut fix: String) { println!("user code") }"#
            ).unwrap();
            let info = (&item_fn).into();
            let tokens = add_parametrize_cases(item_fn.clone(), info);

            let output = ParametrizeOutput::from(tokens);

            let expected = parse2::<ItemMod>(quote! {
                #[cfg(test)]
                mod some {}
            }).unwrap().attrs;

            assert_eq!(expected, output.module.attrs);
        }

        impl ParametrizeInfo {
            fn push_case(&mut self, case: TestCase) {
                self.data.cases.push(case);
            }

            fn extend(&mut self, cases: impl Iterator<Item=TestCase>) {
                self.data.cases.extend(cases);
            }
        }

        impl<'a> From<Vec<Cow<'a, str>>> for TestCase {
            fn from(arguments: Vec<Cow<'a, str>>) -> Self {
                TestCase { args: arguments
                    .iter().map(|a| CaseArg::new(parse_str(a.as_ref().into()).unwrap())).collect(),
                                                      description: None }
            }
        }

        impl<'a> From<Cow<'a, str>> for TestCase {
            fn from(argument: Cow<'a, str>) -> Self {
                vec![argument].into()
            }
        }

        impl<'a> From<&'a str> for TestCase {
            fn from(argument: &'a str) -> Self {
                argument.split(",\n")
                    .map(|s| Cow::from(s))
                    .collect::<Vec<_>>().into()
            }
        }

        fn one_simple_case() -> (ItemFn, ParametrizeInfo) {
            let item_fn = parse_str::<ItemFn>(
                r#"fn test(mut fix: String) { println!("user code") }"#
            ).unwrap();
            let mut info: ParametrizeInfo = (&item_fn).into();
            info.push_case(TestCase::from(r#"String::from("3")"#));
            (item_fn, info)
        }

        fn some_simple_cases(cases: i32) -> (ItemFn, ParametrizeInfo) {
            let item_fn = parse_str::<ItemFn>(
                r#"fn test(mut fix: String) { println!("user code") }"#
            ).unwrap();
            let mut info: ParametrizeInfo = (&item_fn).into();
            info.extend((0..cases).map(|_| TestCase::from(r#"String::from("3")"#)));
            (item_fn, info)
        }

        #[test]
        fn should_add_a_test_case() {
            let (item_fn, info) = one_simple_case();

            let tokens = add_parametrize_cases(item_fn.clone(), info);

            let tests = ParametrizeOutput::from(tokens).get_test_functions();

            assert_eq!(1, tests.len());
            assert!(&tests[0].ident.to_string().starts_with("case_"))
        }

        #[test]
        fn case_number_should_starts_from_1() {
            let (item_fn, info) = one_simple_case();

            let tokens = add_parametrize_cases(item_fn.clone(), info);

            let tests = ParametrizeOutput::from(tokens).get_test_functions();

            assert!(&tests[0].ident.to_string().starts_with("case_1"))
        }

        #[test]
        fn should_add_all_test_cases() {
            let (item_fn, info) = some_simple_cases(5);

            let tokens = add_parametrize_cases(item_fn.clone(), info);

            let tests = ParametrizeOutput::from(tokens).get_test_functions();

            let valid_names = tests.iter()
                .filter(|it| it.ident.to_string().starts_with("case_"));
            assert_eq!(5, valid_names.count())
        }

        #[test]
        fn should_left_pad_case_number_by_zeros() {
            let (item_fn, info) = some_simple_cases(1000);

            let tokens = add_parametrize_cases(item_fn.clone(), info);

            let tests = ParametrizeOutput::from(tokens).get_test_functions();

            let first_name = tests[0].ident.to_string();
            let last_name = tests[999].ident.to_string();

            assert!(first_name.ends_with("_0001"));
            assert!(last_name.ends_with("_1000"));

            let valid_names = tests.iter()
                .filter(|it| it.ident.to_string().len() == first_name.len());
            assert_eq!(1000, valid_names.count())
        }

        #[test]
        fn should_use_description_if_any() {
            let (item_fn, mut info) = one_simple_case();
            let description = "show_this_description";
            info.data.cases[0].description = Some(parse_str::<Ident>(description).unwrap());

            let tokens = add_parametrize_cases(item_fn.clone(), info);

            let tests = ParametrizeOutput::from(tokens).get_test_functions();

            assert!(tests[0].ident.to_string().ends_with(&format!("_{}", description)));
        }
    }

    mod fixture {
        use syn::{ItemFn, ItemStruct, ItemImpl, parse_str, parse2};
        use syn::parse::{Parse, Result, ParseBuffer};
        use crate::render_fixture;

        struct FixtureOutput {
            orig: ItemFn,
            fixture: ItemStruct,
            core_impl: ItemImpl,
            default_impl: ItemImpl,
        }

        impl Parse for FixtureOutput {
            fn parse(input: &ParseBuffer) -> Result<Self> {
                Ok(FixtureOutput {
                    fixture: input.parse()?,
                    core_impl: input.parse()?,
                    default_impl: input.parse()?,
                    orig: input.parse()?,
                })
            }
        }

        fn test_maintains_function_visibility(code: &str) {
            let item_fn = parse_str::<ItemFn>(
                code
            ).unwrap();
            let expected_visibility = item_fn.vis.clone();

            let tokens = render_fixture(item_fn,
                                        Default::default(), Default::default());

            let out: FixtureOutput = parse2(tokens).unwrap();

            assert_eq!(expected_visibility, out.fixture.vis);
            assert_eq!(expected_visibility, out.orig.vis);
        }

        #[test]
        fn should_maintains_pub_visibility() {
            test_maintains_function_visibility(
                r#"pub fn test() { }"#
            );
        }

        #[test]
        fn should_maintains_no_pub_visibility() {
            test_maintains_function_visibility(
                r#"fn test() { }"#
            );
        }
    }
}

