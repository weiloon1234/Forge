use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

/// Expands `#[derive(forge::TS)]` to register the type for TypeScript export.
pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let name = ident.to_string();

    Ok(quote! {
        ::forge::inventory::submit! {
            ::forge::typescript::TsType {
                name: #name,
                export_fn: |dir| <#ident as ::forge::ts_rs::TS>::export_all_to(dir),
            }
        }
    })
}

/// Additional registration for AppEnum types — includes runtime metadata.
pub fn expand_app_enum(input: &DeriveInput) -> TokenStream {
    let ident = &input.ident;
    let name = ident.to_string();

    quote! {
        ::forge::inventory::submit! {
            ::forge::typescript::TsAppEnum {
                name: #name,
                meta_fn: || <#ident as ::forge::ForgeAppEnum>::meta(),
            }
        }
    }
}
