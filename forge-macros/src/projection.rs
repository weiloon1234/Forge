use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

use crate::common::{
    ensure_named_struct, field_name_literal, helper_ident, infer_or_explicit_db_type,
    loaded_inner_type, parse_field_args, require_ident, screaming_const_ident, static_ident,
};

pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let ident = input.ident.clone();
    let fields = ensure_named_struct(&input)?;

    let mut aliases = BTreeSet::new();
    let mut const_defs = Vec::new();
    let mut field_info_entries = Vec::new();
    let mut hydrate_fields = Vec::new();

    for field in &fields.named {
        let field_ident = require_ident(field)?;
        let field_ty = &field.ty;
        let field_args = parse_field_args(field)?;

        if loaded_inner_type(field_ty).is_some() {
            return Err(syn::Error::new_spanned(
                field,
                "Loaded<T> fields are not supported on Projection derives",
            ));
        }

        if field_args.column.is_some() {
            return Err(syn::Error::new_spanned(
                field,
                "Projection derive does not support #[forge(column = ...)]",
            ));
        }

        let alias = field_name_literal(field_ident, &field_args.alias);
        if !aliases.insert(alias.value()) {
            return Err(syn::Error::new_spanned(
                &alias,
                format!("duplicate projection alias `{}`", alias.value()),
            ));
        }

        let db_type = infer_or_explicit_db_type(field_ty, field_args.db_type)?;
        let db_type_tokens = db_type.tokens();
        let const_ident = screaming_const_ident(field_ident);

        let const_expr = if let Some(source) = field_args.source {
            quote!(::forge::ProjectionField::from_source(#alias, #source, #db_type_tokens))
        } else {
            quote!(::forge::ProjectionField::new(#alias, #db_type_tokens))
        };

        const_defs.push(quote! {
            pub const #const_ident: ::forge::ProjectionField<Self, #field_ty> = #const_expr;
        });
        field_info_entries.push(quote!(#ident::#const_ident.info()));
        hydrate_fields.push(quote!(#field_ident: record.decode(#ident::#const_ident.alias())?));
    }

    let fields_static = static_ident("PROJECTION_FIELDS", &ident);
    let hydrate_fn = helper_ident("hydrate_projection", &ident);
    let field_count = field_info_entries.len();

    Ok(quote! {
        impl #ident {
            #(#const_defs)*
        }

        static #fields_static: [::forge::ProjectionFieldInfo; #field_count] =
            [#(#field_info_entries),*];

        fn #hydrate_fn(record: &::forge::DbRecord) -> ::forge::Result<#ident> {
            Ok(#ident {
                #(#hydrate_fields),*
            })
        }

        impl ::forge::Projection for #ident {
            fn projection_meta() -> &'static ::forge::ProjectionMeta<Self> {
                static META: ::std::sync::OnceLock<::forge::ProjectionMeta<#ident>> =
                    ::std::sync::OnceLock::new();
                META.get_or_init(|| ::forge::ProjectionMeta::new(&#fields_static, #hydrate_fn))
            }
        }
    })
}
