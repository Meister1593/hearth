#![feature(proc_macro_quote)]


use proc_macro2::{Literal, Punct, Spacing, Span, TokenStream};
use quote::{quote, ToTokens};
use syn::{ImplItem, ImplItemMethod, Type, Ident, parse_macro_input, FnArg, PatType, Pat, PatIdent, ItemFn};
use syn::parse::{ParseBuffer, ParseStream};
use syn::punctuated::Punctuated;
use syn::token::Comma;


#[proc_macro_attribute]
pub fn impl_wasm_linker(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let impl_item = parse_macro_input!(item as syn::ItemImpl);

    let fn_items = impl_item.items;
    let impl_type = impl_item.self_ty.clone();

    let mut items_within_impl = vec![];
    let mut link_wrapped_fns = vec![];
    let mut wasm_linker_fns = vec![];
    for fn_item in fn_items.clone() {
        items_within_impl.push(
            quote! {
                #fn_item
            }
        );
        handle_fn_item(&mut link_wrapped_fns, &mut wasm_linker_fns, impl_type.clone(), fn_item);
    }



    let return_token_stream: proc_macro::TokenStream = quote! {
        impl #impl_type {
            #(#items_within_impl)*
            #(#link_wrapped_fns)*
        }
        impl <T: AsRef<#impl_type> + Send + 'static> WasmLinker<T> for #impl_type {
            fn add_to_linker(linker: &mut Linker<T>) {
                #(#wasm_linker_fns)*
            }
        }
    }.into();

    println!("{}", return_token_stream);
    return_token_stream
    //quote!{}.into()
}
fn handle_fn_item(link_wrapped_fns: &mut Vec<TokenStream>, wasm_linker_fns: &mut Vec<TokenStream>, impl_type: Box<Type>, fn_item: ImplItem) {
    let fn_method = get_fn_method(fn_item);
    let impl_type = get_impl_type_ident(impl_type);
    let link_fn_ident = get_link_fn_ident(&fn_method);

    let linker_function = generate_linker_function(&link_fn_ident, &fn_method, &impl_type);
    let wasm_linker_fn = generate_add_to_linker_call(&link_fn_ident);
    link_wrapped_fns.push(linker_function);
    wasm_linker_fns.push(wasm_linker_fn);

}
fn generate_linker_function(link_fn_ident: &Ident, fn_method: &ImplItemMethod, impl_type: &Ident) -> TokenStream {
    let link_fn_ident = link_fn_ident.clone();
    let internal_function = generate_internal_function(fn_method, impl_type);
    let func_wrap_call = generate_func_wrap(fn_method, impl_type);
    quote! {
        pub fn #link_fn_ident<T: AsRef<Self> + Send>(linker: &mut Linker<T>) {
            #internal_function
            #func_wrap_call
        }
    }
}
fn generate_internal_function(fn_method: &ImplItemMethod, impl_type: &Ident) -> TokenStream {
    let impl_type = impl_type.clone();
    let is_async = is_async(fn_method);
    let fn_name = get_fn_name(fn_method);
    let internal_args = get_internal_args(fn_method);
    let internal_parameters = get_internal_parameters(fn_method);
    let return_type = fn_method.sig.output.clone();
    match is_async {
        true => {
            quote!{
                async fn #fn_name <T: AsRef<#impl_type> + Send>(#internal_args) #return_type {
                    let this = caller.data().as_ref();
                    this.#fn_name(#internal_parameters).await
                }
            }
        }
        false => {
            quote !{
                fn #fn_name <T: AsRef<#impl_type> + Send>(#internal_args) #return_type {
                    let this = caller.data().as_ref();
                    this.#fn_name(#internal_parameters)
                }
            }
        }
    }
}
fn generate_add_to_linker_call(link_fn_ident: &Ident) -> TokenStream {
    let link_fn_ident = link_fn_ident.clone();
    quote!{
        Self::#link_fn_ident(linker);
    }
}
fn generate_func_wrap(fn_method: &ImplItemMethod, impl_type: &Ident) -> TokenStream {
    let func_wrap_ident = generate_func_wrap_ident(fn_method);
    let module_literal = get_module_literal(impl_type);
    let fn_literal = get_func_wrap_literal(fn_method);
    let closure_call_params = generate_closure_call_params(fn_method);
    let closure_args = generate_closure_args(fn_method);

    let internal_fn_name = get_fn_name(fn_method);
    let fn_call_thing = match is_async(fn_method) {
        true => {
            quote! {
                Box::new(#internal_fn_name(caller, #closure_call_params))
            }
        }
        false => {
            quote! {
                #internal_fn_name(caller, #closure_call_params)
            }
        }
    };
    match has_guest_memory(&get_fn_args(fn_method)) {
        true => {
            quote! {
                linker.#func_wrap_ident(#module_literal, #fn_literal, |#closure_args| {
                    let memory = GuestMemory::from_caller(&mut caller);

                    #fn_call_thing
                }).unwrap();
            }
        }
        false => {
            quote! {
                linker.#func_wrap_ident(#module_literal, #fn_literal, |#closure_args| {
                    #fn_call_thing
                }).unwrap();
            }
        }
    }
}
fn generate_closure_call_params(fn_method: &ImplItemMethod) -> TokenStream {
    get_internal_parameters(fn_method)
}
fn generate_closure_args(fn_method: &ImplItemMethod) -> TokenStream {
    let caller_arg= quote! {
      mut caller: Caller<'_, T>
    };
    let mut fn_args = remove_guest_memory_if_exists(get_fn_args(fn_method));
    quote! {
        #caller_arg, #(#fn_args),*
    }
}
fn generate_func_wrap_ident(fn_method: &ImplItemMethod) -> Ident {
    let is_async = is_async(fn_method);
    let mut num_args = get_fn_args(fn_method).len();
    if has_guest_memory(&get_fn_args(fn_method)) {
        num_args -= 1;
    }
    let str = match is_async {
        true => {
            format!("func_wrap{}_async", num_args)
        }
        false => {
            String::from("func_wrap")
        }
    };
    Ident::new(str.as_str(), Span::call_site())
}
fn get_internal_args(fn_method: &ImplItemMethod) -> TokenStream {
    let caller_arg= quote! {
      mut caller: Caller<'_, T>
    };
    let fn_args = get_fn_args(fn_method);
    quote! {
        #caller_arg, #(#fn_args),*
    }
}
fn get_internal_parameters(fn_method: &ImplItemMethod) -> TokenStream {
    let mut args = get_fn_args(fn_method);
    let args: Vec<_> = args.into_iter().map(|arg| {
        match arg {
            FnArg::Receiver(_) => panic!(),
            FnArg::Typed(typed) => {
                match typed.pat.as_ref() {
                    Pat::Ident(ident) => {
                        Pat::Ident(PatIdent {
                            attrs: vec![],
                            by_ref: None,
                            mutability: None,
                            ident: ident.ident.clone(),
                            subpat: None
                        })
                    }
                    _ => panic!(),
                }
            }
        }
    }).collect();
    quote! {
        #(#args),*
    }
}
fn get_link_fn_ident(fn_method: &ImplItemMethod) -> Ident {
    let fn_name = get_fn_name(fn_method);
    let str = format!("link_{}", fn_name);
    Ident::new(str.as_str(), Span::call_site())
}
fn get_fn_name(fn_method: &ImplItemMethod) -> Ident {
    fn_method.sig.ident.clone()
}
fn get_func_wrap_literal(fn_method: &ImplItemMethod) -> Literal {
    Literal::string(fn_method.sig.ident.to_string().as_str())
}
fn get_module_literal(impl_type_ident: &Ident) -> Literal {
    Literal::string(impl_type_ident.to_string().to_lowercase().as_str())
}
fn get_fn_args(fn_method: &ImplItemMethod) -> Vec<FnArg> {
    let mut args: Vec<FnArg> = fn_method.sig.inputs.iter().map(|arg| {
        arg.clone()
    }).collect();
    // removing the 'self' parameter
    args.remove(0);
    args
}
fn get_impl_type_ident(impl_type: Box<Type>) -> Ident {
    match impl_type.as_ref() {
        Type::Path(path) => {
            path.path.get_ident().unwrap().clone()
        }
        _ => panic!()
    }
}
fn has_guest_memory(fn_args: &Vec<FnArg>) -> bool {
    for fn_arg in fn_args {
        match fn_arg {
            FnArg::Receiver(_) => {}
            FnArg::Typed(typed) => {
                match typed.ty.as_ref() {
                    Type::Path(path) => {
                        for seg in path.path.segments.iter() {
                            if seg.ident.to_string() == "GuestMemory" {
                                return true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    false
}
fn remove_guest_memory_if_exists(fn_args: Vec<FnArg>) -> Vec<FnArg> {
    let mut new_args = vec![];
    for fn_arg in fn_args {
        new_args.push(fn_arg.clone());
        match fn_arg.clone() {
            FnArg::Receiver(_) => {}
            FnArg::Typed(typed) => {
                match typed.ty.as_ref() {
                    Type::Path(path) => {
                        for seg in path.path.segments.iter() {
                            if seg.ident.to_string() == "GuestMemory" {
                                new_args.pop();
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    new_args
}
fn is_async(fn_method: &ImplItemMethod) -> bool {
    fn_method.sig.asyncness.is_some()
}
fn get_fn_method(fn_item: ImplItem) -> ImplItemMethod {
    match fn_item {
        ImplItem::Method(method) => method,
        _ => panic!("there is a non-method item within this impl block"),
    }
}