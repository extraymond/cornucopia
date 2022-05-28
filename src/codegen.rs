use self::utils::{join_comma, join_ln};
use super::prepare_queries::PreparedModule;
use crate::{
    prepare_queries::{PreparedParams, PreparedQuery, PreparedRow},
    type_registrar::{CornucopiaType, TypeRegistrar},
};
use error::Error;
use postgres_types::{Field, Kind};
use std::collections::HashMap;
use std::fmt::Write;

// write! without errors
// Maybe something fancier later
macro_rules! gen {
    ($($t:tt)*) => {{
        write!($($t)*).unwrap();
    }};
}

/// Utils functions to make codegen clearer
mod utils {
    use std::{
        cell::RefCell,
        fmt::{Display, Formatter, Write},
    };

    pub struct Joiner<T, I: IntoIterator<Item = T>, F: Fn(&mut Formatter, T)> {
        sep: char,
        /// FormatWith uses interior mutability because Display::fmt takes &self.
        inner: RefCell<Option<I>>,
        mapper: F,
    }

    impl<T, I: IntoIterator<Item = T>, F: Fn(&mut Formatter, T)> Display for Joiner<T, I, F> {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            let mut first = true;
            for item in self.inner.borrow_mut().take().unwrap().into_iter() {
                if first {
                    first = false;
                } else {
                    f.write_char(self.sep)?;
                }
                (self.mapper)(f, item);
            }
            Ok(())
        }
    }

    pub fn join<T, I: IntoIterator<Item = T>, F: Fn(&mut Formatter, T)>(
        iter: I,
        map: F,
        sep: char,
    ) -> Joiner<T, I, F> {
        Joiner {
            sep,
            inner: RefCell::new(Some(iter)),
            mapper: map,
        }
    }

    pub fn join_comma<T, I: IntoIterator<Item = T>, F: Fn(&mut Formatter, T)>(
        iter: I,
        map: F,
    ) -> Joiner<T, I, F> {
        join(iter, map, ',')
    }

    pub fn join_ln<T, I: IntoIterator<Item = T>, F: Fn(&mut Formatter, T)>(
        iter: I,
        map: F,
    ) -> Joiner<T, I, F> {
        join(iter, map, '\n')
    }
}

// Unused for now, but could be used eventually to error on reserved
// keywords, or support them via raw identifiers.
#[allow(unused)]
fn is_reserved_keyword(s: &str) -> bool {
    [
        "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
        "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
        "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box", "do",
        "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
        "union",
    ]
    .contains(&s)
}

fn domain_brw_fromsql(w: &mut impl Write, struct_name: &str, ty_name: &str, ty_schema: &str) {
    gen!(
        w,
        r#"impl<'a> postgres_types::FromSql<'a> for {struct_name}Borrowed<'a> {{
        fn from_sql(
            _type: &postgres_types::Type,
            buf: &'a [u8],
        ) -> std::result::Result<
            {struct_name}Borrowed<'a>,
            std::boxed::Box<dyn std::error::Error + std::marker::Sync + std::marker::Send>,
        > {{
            let inner = match *_type.kind() {{
                postgres_types::Kind::Domain(ref inner) => inner,
                _ => unreachable!(),
            }};
            let mut buf = buf;
            let _oid = postgres_types::private::read_be_i32(&mut buf)?;
            std::result::Result::Ok({struct_name}Borrowed(
                postgres_types::private::read_value(inner, &mut buf)?))
        }}
        fn accepts(type_: &postgres_types::Type) -> bool {{
            type_.name() == "{ty_name}" && type_.schema() == "{ty_schema}"
        }}
    }}"#
    )
}

fn domain_tosql(
    w: &mut impl Write,
    type_registrar: &TypeRegistrar,
    struct_name: &str,
    inner_ty: &CornucopiaType,
    ty_name: &str,
    is_params: bool,
) {
    let accept_ty = inner_ty.borrowed_rust_ty(type_registrar, Some("'a"), true);
    let post = if is_params { "Borrowed" } else { "Params" };
    gen!(
        w,
        r#"impl <'a> postgres_types::ToSql for {struct_name}{post}<'a> {{
        fn to_sql(
            &self,
            _type: &postgres_types::Type,
            buf: &mut postgres_types::private::BytesMut,
        ) -> std::result::Result<
            postgres_types::IsNull,
            std::boxed::Box<dyn std::error::Error + Sync + Send>,
        > {{
            let type_ = match *_type.kind() {{
                postgres_types::Kind::Domain(ref type_) => type_,
                _ => unreachable!(),
            }};
            postgres_types::ToSql::to_sql(&self.0, type_, buf)
        }}
        fn accepts(type_: &postgres_types::Type) -> bool {{
            if type_.name() != "{ty_name}" {{
                return false;
            }}
            match *type_.kind() {{
                postgres_types::Kind::Domain(ref type_) => <{accept_ty} as postgres_types::ToSql>::accepts(
                    type_
                ),
                _ => false,
            }}
        }}
        fn to_sql_checked(
            &self,
            ty: &postgres_types::Type,
            out: &mut postgres_types::private::BytesMut,
        ) -> std::result::Result<
            postgres_types::IsNull,
            Box<dyn std::error::Error + std::marker::Sync + std::marker::Send>,
        > {{
            postgres_types::__to_sql_checked(self, ty, out)
        }}
    }}"#
    )
}

fn composite_tosql(
    w: &mut impl Write,
    type_registrar: &TypeRegistrar,
    struct_name: &str,
    fields: &[Field],
    ty_name: &str,
    is_params: bool,
) {
    let post = if is_params { "Borrowed" } else { "Params" };
    let nb_fields = fields.len();
    let write_fields = join_ln(fields.iter(), |w, f| {
        let name = f.name();
        gen!(
            w,
            "\"{name}\" => postgres_types::ToSql::to_sql(&self.{name},field.type_(), buf),",
        )
    });
    let accept_fields = join_ln(fields.iter(), |w, f| {
        gen!(
            w,
            "\"{}\" => <{} as postgres_types::ToSql>::accepts(f.type_()),",
            f.name(),
            type_registrar.get(f.type_()).unwrap().borrowed_rust_ty(
                type_registrar,
                Some("'a"),
                true
            )
        )
    });

    gen!(
        w,
        r#"impl<'a> postgres_types::ToSql for {struct_name}{post}<'a> {{
        fn to_sql(
            &self,
            _type: &postgres_types::Type,
            buf: &mut postgres_types::private::BytesMut,
        ) -> std::result::Result<postgres_types::IsNull, std::boxed::Box<std::error::Error + Sync + Send>,> {{
            let fields = match *_type.kind() {{
                postgres_types::Kind::Composite(ref fields) => fields,
                _ => unreachable!(),
            }};
            buf.extend_from_slice(&(fields.len() as i32).to_be_bytes());
            for field in fields {{
                buf.extend_from_slice(&field.type_().oid().to_be_bytes());
                let base = buf.len();
                buf.extend_from_slice(&[0; 4]);
                let r = match field.name() {{
                    {write_fields}
                    _ => unreachable!()
                }};
                let count = match r? {{
                    postgres_types::IsNull::Yes => -1,
                    postgres_types::IsNull::No => {{
                        let len = buf.len() - base - 4;
                        if len > i32::max_value() as usize {{
                            return std::result::Result::Err(std::convert::Into::into(
                                "value too large to transmit",
                            ));
                        }}
                        len as i32
                    }}
                }};
                buf[base..base + 4].copy_from_slice(&count.to_be_bytes());
            }}
            std::result::Result::Ok(postgres_types::IsNull::No)
        }}
        fn accepts(type_: &postgres_types::Type) -> bool {{
            if type_.name() != "{ty_name}" {{
                return false;
            }}
            match *type_.kind() {{
                postgres_types::Kind::Composite(ref fields) => {{
                    if fields.len() != {nb_fields}usize {{
                        return false;
                    }}
                    fields.iter().all(|f| match f.name() {{
                        {accept_fields}
                        _ => false,
                    }})
                }}
                _ => false,
            }}
        }}
        fn to_sql_checked(
            &self,
            ty: &postgres_types::Type,
            out: &mut postgres_types::private::BytesMut,
        ) -> std::result::Result<postgres_types::IsNull, Box<dyn std::error::Error + Sync + Send>> {{
            postgres_types::__to_sql_checked(self, ty, out)
        }}
    }}"#
    );
}

fn composite_fromsql(
    w: &mut impl Write,
    struct_name: &str,
    fields: &[Field],
    ty_name: &str,
    ty_schema: &str,
) {
    let field_names = join_comma(fields, |w, f| gen!(w, "{}", f.name()));
    let read_fields = join_ln(fields.iter().enumerate(), |w, (index, f)| {
        gen!(
            w,
            "let _oid = postgres_types::private::read_be_i32(&mut buf)?;
            let {} = postgres_types::private::read_value(fields[{index}].type_(), &mut buf)?;",
            f.name(),
        )
    });

    gen!(
        w,
        r#"impl<'a> postgres_types::FromSql<'a> for {struct_name}Borrowed<'a> {{
        fn from_sql(
            _type: &postgres_types::Type,
            buf: &'a [u8],
        ) -> Result<{struct_name}Borrowed<'a>, std::boxed::Box<dyn std::error::Error + Sync + Send>> {{
            let fields = match *_type.kind() {{
                postgres_types::Kind::Composite(ref fields) => fields,
                _ => unreachable!(),
            }};
            let mut buf = buf;
            let num_fields = postgres_types::private::read_be_i32(&mut buf)?;
            {read_fields}
            Result::Ok({struct_name}Borrowed {{ {field_names} }})
        }}

        fn accepts(type_: &postgres_types::Type) -> bool {{
            type_.name() == "{ty_name}" && type_.schema() == "{ty_schema}"
        }}
    }}"#
    )
}

fn gen_params_struct(
    w: &mut impl Write,
    type_registrar: &TypeRegistrar,
    module: &PreparedModule,
    params: &PreparedParams,
    is_async: bool,
) {
    let PreparedParams {
        name,
        fields,
        queries,
    } = params;
    let struct_fields = join_comma(fields, |w, p| {
        gen!(
            w,
            "pub {} : {}",
            p.name,
            p.ty.borrowed_rust_ty(type_registrar, Some("'a"), true)
        )
    });
    let is_copy = fields.iter().all(|a| a.ty.is_copy);
    let (copy, lifetime, fn_lifetime) = if is_copy {
        ("Clone,Copy,", "", "'a,")
    } else {
        ("", "<'a>", "")
    };
    let (backend, client_mut) = if is_async {
        ("tokio_postgres", "")
    } else {
        ("postgres", "mut")
    };
    let params_methods = join_ln(queries, |w, idx| {
        let PreparedQuery {
            name, params, row, ..
        } = module.queries.get_index(*idx).unwrap().1;

        let param_values = join_comma(params, |w, p| gen!(w, "&self.{}", p.name));
        let (fn_async, fn_await) = if row.is_none() && is_async {
            ("async", ".await")
        } else {
            ("", "")
        };
        let ret_type = if let Some((idx, _)) = row {
            let name = &module.rows.get_index(*idx).unwrap().1.name;
            let nb_params = params.len();
            format!("{name}Query<'a, C, {name}, {nb_params}>")
        } else {
            format!("Result<u64, {backend}::Error>")
        };
        // Generate params struct
        gen!(w,
            "pub {fn_async} fn {name}<{fn_lifetime}C: GenericClient>(&'a self, client: &'a {client_mut} C) -> {ret_type} {{
                {name}(client, {param_values}){fn_await}
            }}")
    });
    gen!(
        w,
        "#[derive(Debug, {copy})]
        pub struct {name}{lifetime} {{ {struct_fields} }}
        impl {lifetime} {name} {lifetime} {{ {params_methods} }}"
    );
}

fn gen_row_structs(
    w: &mut impl Write,
    type_registrar: &TypeRegistrar,
    row: &PreparedRow,
    is_async: bool,
) {
    let PreparedRow {
        name,
        fields,
        is_copy,
    } = row;
    {
        // Generate row struct
        let struct_fields = join_comma(fields, |w, col| {
            let col_name = &col.name;
            let col_ty = if col.is_nullable {
                format!("Option<{}>", col.ty.rust_path_from_queries)
            } else {
                col.ty.rust_path_from_queries.clone()
            };
            gen!(w, "pub {col_name} : {col_ty}")
        });
        let copy = if *is_copy { "Copy" } else { "" };
        gen!(
            w,
            "#[derive(Debug, Clone, PartialEq,{copy})] pub struct {name} {{ {struct_fields} }}",
        );

        if !is_copy {
            let struct_fields = join_comma(fields, |w, col| {
                let col_name = &col.name;
                let col_ty = if col.is_nullable {
                    format!(
                        "Option<{}>",
                        col.ty.borrowed_rust_ty(type_registrar, Some("'a"), false)
                    )
                } else {
                    col.ty.borrowed_rust_ty(type_registrar, Some("'a"), false)
                };
                gen!(w, "pub {col_name} : {col_ty}")
            });
            let fields_names = join_comma(fields, |w, f| gen!(w, "{}", f.name));
            let borrowed_fields_to_owned = join_comma(fields, |w, f| {
                let field_name = &f.name;
                let owned_value = if f.ty.is_copy {
                    String::new()
                } else {
                    format!(": {}", f.ty.owning_call(&f.name, f.is_nullable))
                };
                gen!(w, "{field_name} {owned_value}")
            });
            gen!(
                w,
                "pub struct {name}Borrowed<'a> {{ {struct_fields} }}
            impl<'a> From<{name}Borrowed<'a>> for {name} {{
                fn from({name}Borrowed {{ {fields_names} }}: {name}Borrowed<'a>) -> Self {{
                    Self {{ {borrowed_fields_to_owned} }}
                }}
            }}"
            );
        };
    }
    {
        // Generate query struct
        let nb_fields = fields.len();
        let (borrowed_str, lifetime) = if *is_copy {
            ("", "")
        } else {
            ("Borrowed", "<'b>")
        };
        let (client_mut, fn_async, fn_await, backend, collect, raw_type, raw_pre, raw_post) =
            if is_async {
                (
                    "",
                    "async",
                    ".await",
                    "tokio_postgres",
                    "try_collect().await",
                    "futures::Stream",
                    "",
                    ".into_stream()",
                )
            } else {
                (
                    "mut",
                    "",
                    "",
                    "postgres",
                    "collect()",
                    "Iterator",
                    ".iterator()",
                    "",
                )
            };
        let get_fields = join_comma(fields.iter().enumerate(), |w, (index, f)| {
            gen!(w, "{}: row.get(indexes[{index}])", f.name)
        });

        gen!(w,"
        pub struct {name}Query<'a, C: GenericClient, T, const N: usize> {{
            client: &'a {client_mut} C,
            params: [&'a (dyn postgres_types::ToSql + Sync); N],
            indexes: &'static [usize; {nb_fields}], 
            query: &'static str,
            mapper: fn({name}{borrowed_str}) -> T,
        }}
        impl<'a, C, T:'a, const N: usize> {name}Query<'a, C, T, N> where C: GenericClient {{
            pub fn map<R>(self, mapper: fn({name}{borrowed_str}) -> R) -> {name}Query<'a,C,R,N> {{
                {name}Query {{
                    client: self.client,
                    params: self.params,
                    query: self.query,
                    indexes: self.indexes,
                    mapper,
                }}
            }}

            pub fn extractor<'b>(row: &'b {backend}::row::Row, indexes: &'static [usize;{nb_fields}]) -> {name}{borrowed_str}{lifetime} {{
                {name}{borrowed_str} {{ {get_fields} }}
            }}
        
            pub {fn_async} fn stmt(&{client_mut} self) -> Result<{backend}::Statement, {backend}::Error> {{
                self.client.prepare(self.query){fn_await}
            }}
        
            pub {fn_async} fn one({client_mut} self) -> Result<T, {backend}::Error> {{
                let stmt = self.stmt(){fn_await}?;
                let row = self.client.query_one(&stmt, &self.params){fn_await}?;
                Ok((self.mapper)(Self::extractor(&row, self.indexes)))
            }}
        
            pub {fn_async} fn vec(self) -> Result<Vec<T>, {backend}::Error> {{
                self.stream(){fn_await}?.{collect}
            }}
        
            pub {fn_async} fn opt({client_mut} self) -> Result<Option<T>, {backend}::Error> {{
                let stmt = self.stmt(){fn_await}?;
                Ok(self
                    .client
                    .query_opt(&stmt, &self.params)
                    {fn_await}?
                    .map(|row| (self.mapper)(Self::extractor(&row, self.indexes))))
            }}
        
            pub {fn_async} fn stream(
                {client_mut} self,
            ) -> Result<impl {raw_type}<Item = Result<T, {backend}::Error>> + 'a, {backend}::Error> {{
                let stmt = self.stmt(){fn_await}?;
                let stream = self
                    .client
                    .query_raw(&stmt, cornucopia_client::slice_iter(&self.params))
                    {fn_await}?
                    {raw_pre}
                    .map(move |res| res.map(|row| (self.mapper)(Self::extractor(&row, self.indexes))))
                    {raw_post};
                Ok(stream)
            }}
        }}")
    }
}

fn gen_query_fn(
    w: &mut impl Write,
    type_registrar: &TypeRegistrar,
    module: &PreparedModule,
    query: &PreparedQuery,
    is_async: bool,
) {
    let PreparedQuery {
        name,
        params,
        row,
        sql,
    } = query;

    let (client_mut, fn_async, fn_await, backend) = if is_async {
        ("", "async", ".await", "tokio_postgres")
    } else {
        ("mut", "", "", "postgres")
    };

    if let Some((idx, index)) = row {
        let row_name = &module.rows.get_index(*idx).unwrap().1.name;
        // Query fn
        let param_list = join_comma(params, |w, p| {
            let param_name = &p.name;
            let borrowed_rust_ty = p.ty.borrowed_rust_ty(type_registrar, None, true);
            gen!(w, "{param_name} : &'a {borrowed_rust_ty}",)
        });
        let index_str = join_comma(index, |w, p| gen!(w, "{p}"));
        let nb_params = params.len();
        let param_names = join_comma(params, |w, p| gen!(w, "{}", p.name));
        let client_mut = if is_async { "" } else { "mut" };
        gen!(w,
            "pub fn {name}<'a, C: GenericClient>(client: &'a {client_mut} C, {param_list}) -> {row_name}Query<'a,C, {row_name}, {nb_params}> {{
            {row_name}Query {{
                client,
                params: [{param_names}],
                query: \"{sql}\",
                indexes: &[{index_str}],
                mapper: |it| {row_name}::from(it),
            }}
        }}",
        );
    } else {
        // Execute fn
        let param_list = join_comma(params, |w, p| {
            let borrowed_rust_ty = p.ty.borrowed_rust_ty(type_registrar, None, true);
            gen!(w, "{} : &'a {borrowed_rust_ty}", p.name)
        });
        let param_names = join_comma(params, |w, p| gen!(w, "{}", p.name));
        gen!(w,
            "pub {fn_async} fn {name}<'a, C: GenericClient>(client: &'a {client_mut} C, {param_list}) -> Result<u64, {backend}::Error> {{
                let stmt = client.prepare(\"{sql}\"){fn_await}?;
                client.execute(&stmt, &[{param_names}]){fn_await}
            }}"
        )
    }
}

/// Generates type definitions for custom user types. This includes domains, composites and enums.
/// If the type is not `Copy`, then a Borrowed version will be generated.
fn gen_custom_type(w: &mut impl Write, type_registrar: &TypeRegistrar, ty: &CornucopiaType) {
    let ty_name = ty.pg_ty.name();
    let ty_schema = ty.pg_ty.schema();
    let struct_name = &ty.rust_ty_name;
    let copy = if ty.is_copy { "Copy," } else { "" };
    match &ty.pg_ty.kind() {
        Kind::Enum(variants) => {
            let variants_str = variants.join(",");
            gen!(w,
                "#[derive(Debug, postgres_types::ToSql, postgres_types::FromSql, Clone, Copy, PartialEq, Eq)]
                #[postgres(name = \"{ty_name}\")]
                pub enum {struct_name} {{ {variants_str} }}",
            )
        }
        Kind::Domain(domain_inner_ty) => {
            let inner_ty = type_registrar.get(domain_inner_ty).unwrap();
            let inner_rust_path_from_ty = &inner_ty.rust_path_from_types;
            gen!(
                w,
                "#[derive(Debug, {copy}Clone, PartialEq, postgres_types::ToSql,postgres_types::FromSql)]
                #[postgres(name = \"{ty_name}\")]
                pub struct {struct_name} (pub {inner_rust_path_from_ty});"
            );
            if !ty.is_copy {
                let brw_fields_str = inner_ty.borrowed_rust_ty(type_registrar, Some("'a"), false);
                let inner_value = inner_ty.owning_call("inner", false);
                gen!(
                    w,
                    "#[derive(Debug)]
                    pub struct {struct_name}Borrowed<'a> (pub {brw_fields_str});
                    impl<'a> From<{struct_name}Borrowed<'a>> for {struct_name} {{
                        fn from(
                            {struct_name}Borrowed (inner): {struct_name}Borrowed<'a>,
                        ) -> Self {{ Self({inner_value}) }}
                    }}"
                );
                domain_brw_fromsql(w, struct_name, ty_name, ty_schema);
                if !ty.is_params {
                    let params_fields_str =
                        inner_ty.borrowed_rust_ty(type_registrar, Some("'a"), true);
                    gen!(
                        w,
                        "#[derive(Debug, Clone)]
                        pub struct {struct_name}Params<'a>(pub {params_fields_str});",
                    );
                }
                domain_tosql(
                    w,
                    type_registrar,
                    struct_name,
                    inner_ty,
                    ty_name,
                    ty.is_params,
                );
            }
        }
        Kind::Composite(fields) => {
            let fields_str = join_comma(fields, |w, f| {
                let f_ty = type_registrar.get(f.type_()).unwrap();
                gen!(w, "pub {} : {}", f.name(), f_ty.rust_path_from_types)
            });
            gen!(
                w,
                "#[derive(Debug,postgres_types::ToSql,postgres_types::FromSql,{copy} Clone, PartialEq)]
                #[postgres(name = \"{ty_name}\")]
                pub struct {struct_name} {{ {fields_str} }}"
            );
            if !ty.is_copy {
                let borrowed_fields_str = join_comma(fields, |w, f| {
                    let f_ty = type_registrar.get(f.type_()).unwrap();
                    gen!(
                        w,
                        "pub {} : {}",
                        f.name(),
                        f_ty.borrowed_rust_ty(type_registrar, Some("'a"), false)
                    )
                });
                let field_names = join_comma(fields, |w, f| gen!(w, "{}", f.name()));
                let field_values = join_comma(fields, |w, f| {
                    let f_ty = type_registrar.get(f.type_()).unwrap();
                    gen!(
                        w,
                        "{} {}",
                        f.name(),
                        if f_ty.is_copy {
                            String::new()
                        } else {
                            format!(": {}", f_ty.owning_call(f.name(), false))
                        }
                    )
                });
                gen!(
                    w,
                    "#[derive(Debug)]
                    pub struct {struct_name}Borrowed<'a> {{ {borrowed_fields_str} }}
                    impl<'a> From<{struct_name}Borrowed<'a>> for {struct_name} {{
                        fn from(
                            {struct_name}Borrowed {{
                            {field_names}
                            }}: {struct_name}Borrowed<'a>,
                        ) -> Self {{ Self {{ {field_values} }} }}
                    }}",
                );
                composite_fromsql(w, struct_name, fields, ty_name, ty_schema);
                if !ty.is_params {
                    let params_fields_str = join_comma(fields, |w, f| {
                        let f_ty = type_registrar.get(f.type_()).unwrap();
                        gen!(
                            w,
                            "pub {} : {}",
                            f.name(),
                            f_ty.borrowed_rust_ty(type_registrar, Some("'a"), true)
                        )
                    });
                    gen!(
                        w,
                        "#[derive(Debug, Clone)]
                        pub struct {struct_name}Params<'a> {{ {params_fields_str} }}",
                    );
                }
                composite_tosql(
                    w,
                    type_registrar,
                    struct_name,
                    fields,
                    ty_name,
                    ty.is_params,
                );
            }
        }
        _ => unreachable!(),
    }
}

fn gen_type_modules(w: &mut impl Write, type_registrar: &TypeRegistrar) -> Result<(), Error> {
    // Group the custom types by schema name
    let mut modules = HashMap::<String, Vec<CornucopiaType>>::new();
    for ((schema, _), ty) in &type_registrar.custom_types {
        match modules.entry(schema.to_owned()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().push(ty.clone());
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(vec![ty.clone()]);
            }
        }
    }
    // Generate each module
    let modules_str = join_ln(modules, |w, (mod_name, tys)| {
        let tys_str = join_ln(tys, |w, ty| gen_custom_type(w, type_registrar, &ty));
        gen!(w, "pub mod {mod_name} {{ {tys_str} }}")
    });

    gen!(w, "pub mod types {{ {modules_str} }}");
    Ok(())
}

pub(crate) fn generate(
    type_registrar: &TypeRegistrar,
    modules: Vec<PreparedModule>,
    is_async: bool,
) -> Result<String, Error> {
    let import = if is_async {
        "use futures::{{StreamExt, TryStreamExt}};use cornucopia_client::GenericClient;"
    } else {
        "use postgres::fallible_iterator::FallibleIterator;use postgres::GenericClient;"
    };
    let mut buff = "// This file was generated with `cornucopia`. Do not modify.
    #![allow(clippy::all)]
    #![allow(unused_variables)]
    #![allow(unused_imports)]
    #![allow(dead_code)]
    "
    .to_string();
    // Generate database type
    gen_type_modules(&mut buff, type_registrar)?;
    // Generate queries
    let query_modules = join_ln(modules, |w, module| {
        let queries_string = join_ln(module.queries.values(), |w, query| {
            gen_query_fn(w, type_registrar, &module, query, is_async)
        });
        let params_string = join_ln(module.params.values(), |w, it| {
            gen_params_struct(w, type_registrar, &module, it, is_async)
        });
        let rows_string = join_ln(module.rows.values(), |w, query| {
            gen_row_structs(w, type_registrar, query, is_async)
        });
        gen!(
            w,
            "pub mod {} {{ {import} {params_string} {rows_string} {queries_string} }}",
            module.name
        )
    });
    gen!(&mut buff, "pub mod queries {{ {} }}", query_modules);

    Ok(prettyplease::unparse(&syn::parse_str(&buff)?))
}

pub(crate) mod error {
    use thiserror::Error as ThisError;

    #[derive(Debug, ThisError)]
    #[error("{0}")]
    pub enum Error {
        Io(#[from] WriteFileError),
        Fmt(#[from] syn::parse::Error),
    }

    #[derive(Debug, ThisError)]
    #[error("Error while trying to write to destination file \"{path}\": {err}.")]
    pub struct WriteFileError {
        pub(crate) err: std::io::Error,
        pub(crate) path: String,
    }
}
