use proc_macro::TokenStream;

mod common;
mod model;
mod projection;

#[proc_macro_derive(Model, attributes(forge))]
pub fn derive_model(input: TokenStream) -> TokenStream {
    expand(input, model::expand)
}

#[proc_macro_derive(Projection, attributes(forge))]
pub fn derive_projection(input: TokenStream) -> TokenStream {
    expand(input, projection::expand)
}

fn expand(
    input: TokenStream,
    f: fn(syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream>,
) -> TokenStream {
    match syn::parse(input).and_then(f) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}
