use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{parse_macro_input, spanned::Spanned, Attribute, Data, DeriveInput, Fields, ItemFn, LitStr};

#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // 1. Parse the user's function
    let mut input_fn = parse_macro_input!(item as ItemFn);

    // 2. Rename the user's function so we can call it from our wrappers
    // We change "main" to "__engine_user_main"
    let user_main_ident = syn::Ident::new("__engine_user_main", input_fn.sig.ident.span());
    input_fn.sig.ident = user_main_ident.clone();

    // 3. Generate the wrapper code
    let expanded = quote! {
        // Emit the user's original function (renamed)
        #input_fn

        // --- Native Entry Point ---
        #[cfg(not(target_arch = "wasm32"))]
        fn main(){
            // Simply call the user's function
            __engine_user_main()
        }

        // --- WASM Entry Point ---
        #[cfg(target_arch = "wasm32")]
        use wasm_bindgen::prelude::*;
        #[cfg(target_arch = "wasm32")]
        #[wasm_bindgen]
        pub fn __engine_start(config: JsValue) -> Result<(), JsValue>{
            // parse the optional startup config, then run the game
            efeengine_core::config::__start_from_js(config)?;
            __engine_user_main();
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        #[wasm_bindgen(start)]
        pub fn main(){
            // init the game but not start it for workers to not start


        }
    };

    TokenStream::from(expanded)
}

/// `#[derive(ScriptType)]`: implements `efeengine_core::engine::types::ScriptType` for a
/// `#[repr(C)]` struct with named fields, describing its byte layout as an
/// `efevm::StructDesc` (offsets from `offset_of!`, field types from `ScriptField`).
///
/// Attributes:
/// - `#[script(name = "Other")]` on the struct: the stable name (default: the Rust name).
/// - `#[script(entity)]` on a `u64` (or `[u64; N]`) field: the field holds entity handles.
///
/// `///` doc comments of the struct and its fields become the `doc` strings.
#[proc_macro_derive(ScriptType, attributes(script))]
pub fn derive_script_type(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match script_type(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn script_type(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(input.generics.span(), "ScriptType cannot be derived for a generic type"));
    }
    let data = match &input.data {
        Data::Struct(s) => s,
        Data::Enum(e) => return Err(syn::Error::new(e.enum_token.span, "ScriptType needs a struct with named fields, not an enum")),
        Data::Union(u) => return Err(syn::Error::new(u.union_token.span, "ScriptType needs a struct with named fields, not a union")),
    };
    let fields = match &data.fields {
        Fields::Named(f) => f,
        Fields::Unnamed(f) => return Err(syn::Error::new(f.span(), "ScriptType needs named fields; tuple structs are not supported")),
        Fields::Unit => return Err(syn::Error::new(ident.span(), "ScriptType needs named fields; unit structs are not supported")),
    };
    if !has_repr_c(&input.attrs)? {
        return Err(syn::Error::new(ident.span(), "ScriptType needs `#[repr(C)]` so the field layout is fixed"));
    }

    let mut name = ident.to_string();
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("script")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let lit: LitStr = meta.value()?.parse()?;
                name = lit.value();
                Ok(())
            } else {
                Err(meta.error("unknown struct attribute; expected `#[script(name = \"...\")]`"))
            }
        })?;
    }
    let struct_doc = doc_string(&input.attrs);

    let mut pushes = Vec::new();
    for f in &fields.named {
        let fname = f.ident.as_ref().expect("named field");
        let fname_str = fname.to_string();
        let ty = &f.ty;
        let mut entity = false;
        for attr in f.attrs.iter().filter(|a| a.path().is_ident("script")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("entity") {
                    entity = true;
                    Ok(())
                } else {
                    Err(meta.error("unknown field attribute; expected `#[script(entity)]`"))
                }
            })?;
        }
        let doc = doc_string(&f.attrs);
        let prim = if entity {
            quote_spanned! {ty.span()=>
                {
                    const _: () = ::core::assert!(
                        ::core::matches!(<#ty as ::efeengine_core::engine::types::ScriptField>::PRIM, ::efeengine_core::efevm::Prim::U64),
                        "#[script(entity)] needs a u64 field (or an array of u64)"
                    );
                    ::efeengine_core::efevm::Prim::Entity
                }
            }
        } else {
            quote_spanned! {ty.span()=> <#ty as ::efeengine_core::engine::types::ScriptField>::PRIM }
        };
        let count = quote_spanned! {ty.span()=> <#ty as ::efeengine_core::engine::types::ScriptField>::COUNT };
        pushes.push(quote! {
            d = d.field(
                ::efeengine_core::efevm::FieldDesc::new(
                    #fname_str,
                    ::core::mem::offset_of!(Self, #fname) as u32,
                    #prim,
                    #count,
                )
                .with_doc(#doc),
            );
        });
    }

    Ok(quote! {
        impl ::efeengine_core::engine::types::ScriptType for #ident {
            const NAME: &'static str = #name;
            fn layout() -> ::efeengine_core::efevm::StructDesc {
                let mut d = ::efeengine_core::efevm::StructDesc::new(
                    #name,
                    ::core::mem::size_of::<Self>() as u32,
                    ::core::mem::align_of::<Self>() as u32,
                )
                .doc(#struct_doc);
                #(#pushes)*
                d
            }
        }
    })
}

/// `true` if `#[repr(...)]` lists `C` (other hints such as `align(16)` may follow).
fn has_repr_c(attrs: &[Attribute]) -> syn::Result<bool> {
    let mut c = false;
    for attr in attrs.iter().filter(|a| a.path().is_ident("repr")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("C") {
                c = true;
            } else if meta.input.peek(syn::token::Paren) {
                // e.g. align(16): skip its arguments
                let _content;
                syn::parenthesized!(_content in meta.input);
            }
            Ok(())
        })?;
    }
    Ok(c)
}

/// `///` lines, each trimmed, joined with `\n`, trimmed as a whole.
fn doc_string(attrs: &[Attribute]) -> String {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value().trim().to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    lines.join("\n").trim().to_string()
}
