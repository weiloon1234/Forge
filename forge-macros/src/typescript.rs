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

/// Additional registration for AppEnum types — includes runtime values.
pub fn expand_enum_values(input: &DeriveInput) -> TokenStream {
    let ident = &input.ident;
    let name = ident.to_string();

    quote! {
        ::forge::inventory::submit! {
            ::forge::typescript::TsEnumValues {
                name: #name,
                values_fn: || {
                    <#ident as ::forge::ForgeAppEnum>::options()
                        .iter()
                        .map(|opt| {
                            match &opt.value {
                                ::forge::EnumKey::String(s) => s.clone(),
                                ::forge::EnumKey::Int(i) => i.to_string(),
                            }
                        })
                        .collect()
                },
            }
        }
    }
}
