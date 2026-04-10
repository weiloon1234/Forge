use std::collections::BTreeSet;

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{DeriveInput, LitStr};

use crate::common::{
    ensure_named_struct, field_name_literal, helper_ident, infer_or_explicit_db_type,
    loaded_inner_type, option_inner_type, parse_field_args, parse_model_args, require_ident,
    screaming_const_ident, static_ident,
};

pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let ident = input.ident.clone();
    let fields = ensure_named_struct(&input)?;
    let args = parse_model_args(&input.attrs)?;

    let table = args.model.ok_or_else(|| {
        syn::Error::new_spanned(&ident, "missing #[forge(model = ...)] attribute")
    })?;
    let explicit_primary_key = args.primary_key.is_some();
    let primary_key = args
        .primary_key
        .unwrap_or_else(|| LitStr::new("id", Span::call_site()));
    let lifecycle = args
        .lifecycle
        .unwrap_or_else(|| syn::parse_quote!(::forge::NoModelLifecycle));

    let mut column_names = BTreeSet::new();
    let mut persisted_column_names = Vec::new();
    let mut const_defs = Vec::new();
    let mut column_info_entries = Vec::new();
    let mut hydrate_fields = Vec::new();
    let mut primary_key_field_ident = None;
    let mut primary_key_const_ident = None;

    for field in &fields.named {
        let field_ident = require_ident(field)?;
        let field_ty = &field.ty;
        let field_args = parse_field_args(field)?;

        if loaded_inner_type(field_ty).is_some() {
            if field_args.column.is_some()
                || field_args.alias.is_some()
                || field_args.source.is_some()
                || field_args.db_type.is_some()
            {
                return Err(syn::Error::new_spanned(
                    field,
                    "Loaded<T> fields cannot declare forge field attributes",
                ));
            }
            hydrate_fields.push(quote!(#field_ident: ::forge::Loaded::Unloaded));
            continue;
        }

        if field_args.alias.is_some() || field_args.source.is_some() {
            return Err(syn::Error::new_spanned(
                field,
                "Model derive does not support #[forge(alias = ...)] or #[forge(source = ...)]",
            ));
        }

        let column_name = field_name_literal(field_ident, &field_args.column);
        if !column_names.insert(column_name.value()) {
            return Err(syn::Error::new_spanned(
                &column_name,
                format!("duplicate column name `{}`", column_name.value()),
            ));
        }
        persisted_column_names.push(column_name.value());

        let db_type = infer_or_explicit_db_type(field_ty, field_args.db_type)?;
        let db_type_tokens = db_type.tokens();
        let const_ident = screaming_const_ident(field_ident);
        let is_optional = option_inner_type(field_ty).is_some();
        let is_primary_key = column_name.value() == primary_key.value();
        if is_primary_key {
            primary_key_field_ident = Some(field_ident.clone());
            primary_key_const_ident = Some(const_ident.clone());
        }

        if is_primary_key && is_optional {
            return Err(syn::Error::new_spanned(
                field,
                "primary key fields cannot use Option<T> on Model derives",
            ));
        }

        const_defs.push(quote! {
            pub const #const_ident: ::forge::Column<Self, #field_ty> =
                ::forge::Column::new(#table, #column_name, #db_type_tokens);
        });
        column_info_entries.push(quote!(#ident::#const_ident.info()));
        hydrate_fields.push(quote!(#field_ident: record.decode_column(#ident::#const_ident)?));
    }

    if !persisted_column_names
        .iter()
        .any(|name| name == &primary_key.value())
    {
        if explicit_primary_key {
            return Err(syn::Error::new_spanned(
                &primary_key,
                format!(
                    "primary_key `{}` does not match any persisted field",
                    primary_key.value()
                ),
            ));
        }

        return Err(syn::Error::new_spanned(
            &ident,
            "Model derive requires an `id` field or an explicit #[forge(primary_key = \"...\")] attribute",
        ));
    }

    let columns_static = static_ident("COLUMNS", &ident);
    let hydrate_fn = helper_ident("hydrate", &ident);
    let column_count = column_info_entries.len();
    let primary_key_field_ident = primary_key_field_ident.ok_or_else(|| {
        syn::Error::new_spanned(
            &ident,
            "Model derive requires a resolvable primary key field",
        )
    })?;
    let primary_key_const_ident = primary_key_const_ident.ok_or_else(|| {
        syn::Error::new_spanned(
            &ident,
            "Model derive requires a resolvable primary key column constant",
        )
    })?;

    Ok(quote! {
        impl #ident {
            #(#const_defs)*

            pub fn query() -> ::forge::ModelQuery<Self> {
                <Self as ::forge::Model>::model_query()
            }

            pub fn create() -> ::forge::CreateModel<Self> {
                <Self as ::forge::Model>::model_create()
            }

            pub fn create_many() -> ::forge::CreateManyModel<Self> {
                <Self as ::forge::Model>::model_create_many()
            }

            pub fn update() -> ::forge::UpdateModel<Self> {
                <Self as ::forge::Model>::model_update()
            }

            pub fn delete() -> ::forge::DeleteModel<Self> {
                <Self as ::forge::Model>::model_delete()
            }
        }

        static #columns_static: [::forge::ColumnInfo; #column_count] = [#(#column_info_entries),*];

        fn #hydrate_fn(record: &::forge::DbRecord) -> ::forge::Result<#ident> {
            Ok(#ident {
                #(#hydrate_fields),*
            })
        }

        impl ::forge::Model for #ident {
            type Lifecycle = #lifecycle;

            fn table_meta() -> &'static ::forge::TableMeta<Self> {
                static TABLE: ::std::sync::OnceLock<::forge::TableMeta<#ident>> =
                    ::std::sync::OnceLock::new();
                TABLE.get_or_init(|| {
                    ::forge::TableMeta::new(#table, &#columns_static, #primary_key, #hydrate_fn)
                })
            }
        }

        impl ::forge::PersistedModel for #ident {
            fn persisted_condition(&self) -> ::forge::Condition {
                #ident::#primary_key_const_ident
                    .eq(::core::clone::Clone::clone(&self.#primary_key_field_ident))
            }
        }
    })
}
