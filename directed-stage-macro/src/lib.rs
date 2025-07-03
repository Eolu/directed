mod parse;

use parse::*;
use proc_macro::TokenStream;
use proc_macro_error::proc_macro_error;
use proc_macro2::Span;
use proc_macro2_diagnostics::Diagnostic;
use quote::quote_spanned;
use syn::{ItemFn, Token, parse_macro_input};

/// A macro that wraps a function with the standardized interface:
/// TODO: More in-depth docs here
#[proc_macro_attribute]
#[proc_macro_error]
pub fn stage(attr: TokenStream, item: TokenStream) -> proc_macro::TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    let meta_args = parse_macro_input!(attr as StageArgs);
    let stage_config = StageConfig::from_args(&input_fn, &meta_args).unwrap();
    match generate_stage_impl(stage_config) {
        Ok(tokens) => tokens.into(),
        Err(diag) => diag.emit_as_expr_tokens().into(),
    }
}

/// Get a type, squash the &
fn true_type(ty: &syn::Type) -> &syn::Type {
    if let syn::Type::Reference(ty) = ty {
        &*ty.elem
    } else {
        ty
    }
}

/// This code is used by the wrapper function - it downcasts type-erased
/// function parameters so that the user-facing function can be called with
/// concrete types.
fn generate_extraction_code(
    inputs: &[InputParam],
    cache_strategy: (CacheStrategy, Span),
) -> Result<Vec<proc_macro2::TokenStream>, Diagnostic> {
    Ok(inputs.iter().map(|input| {
        let arg_name = &input.clean_name;
        let arg_type = true_type(&input.ty);
        let clean_arg_name = input.clean_name.to_string();
        let reeval_name = quote::format_ident!("{}_reevaluation_rule", clean_arg_name);
        let input_span = input.span.clone();

        match cache_strategy {
            (CacheStrategy::None, _span) => quote_spanned! {input_span=>
                // Non-transparent functions never clone, always move
                let (#arg_name, #reeval_name): (#arg_type, directed::ReevaluationRule) = {
                    match inputs.#arg_name.take() {
                        Some((arg, reeval_name)) => (arg, reeval_name),
                        None => return Err(directed::InjectionError::InputNotFound(Self::SHAPE.inputs.iter().find(|field| field.name == #clean_arg_name)))
                    }
                };
            },
            (CacheStrategy::Last, _span) | (CacheStrategy::All, _span) => quote_spanned! {input_span=>
                let (#arg_name, #reeval_name): (#arg_type, directed::ReevaluationRule) = {
                    match &inputs.#arg_name {
                        Some((arg, reeval_name)) => (arg.clone(), *reeval_name),
                        None => return Err(directed::InjectionError::InputNotFound(Self::SHAPE.inputs.iter().find(|field| field.name == #clean_arg_name)))
                    }
                };
            },
        }
    }).collect())
}

/// This generates the code that uses the output of a parent node to set the
/// input of a child node.
fn input_injection(
    stage_name: &syn::Ident,
    inputs: &[InputParam],
) -> Result<proc_macro2::TokenStream, Diagnostic> {
    let mut match_arms = Vec::new();

    // Build match arms from inputs
    for input in inputs.iter() {
        let clean_arg_name = input.clean_name.to_string();
        let span = input.span.clone();

        match_arms.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                // Cast node to the concrete type
                let node = node.as_any_mut()
                    .downcast_mut::<directed::Node<Self>>()
                    .ok_or_else(|| directed::InjectionError::InputTypeMismatch(input))?;
                
                // We need to handle different cases based on parent's reevaluation rule
                if parent.reeval_rule() == directed::ReevaluationRule::Move {
                    inject_move(node, parent, output, input)?;
                } else {
                    inject_clone(node, parent, output, input)?;
                }
                
                Ok(())
            }
        });
    }

    // Add the default case
    let default_case = quote_spanned! {Span::call_site()=>
        Some(name) => {
            // TODO: Verify the unwrap in unreachable
            Err(directed::InjectionError::InputNotFound(#stage_name::SHAPE.inputs.iter().find(|field| field.name == name)))
        },
        None => Ok(()) // This means there's a connection with no data associated
    };
    match_arms.push(default_case);

    // Generate the helper functions that work with concrete types
    let inject_helpers = generate_inject_helpers(stage_name, &inputs)?;

    Ok(quote_spanned! {Span::call_site()=>
        #inject_helpers

        let result: Result<(), directed::InjectionError> = match input.map(|input| input.name) {
            #(#match_arms)*
        };
        result
    })
}

fn generate_inject_helpers(
    stage_name: &syn::Ident,
    inputs: &[InputParam],
) -> Result<proc_macro2::TokenStream, Diagnostic> {
    let mut inject_move_arms = Vec::new();
    let mut inject_clone_arms = Vec::new();

    for input in inputs.iter() {
        let arg_name = &input.clean_name;
        let clean_arg_name = &input.clean_name.to_string();
        let arg_type = true_type(&input.ty);
        let span = input.span.clone();

        inject_move_arms.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                // For opaque parent, we need to remove the output
                let output_val = parent.outputs_mut()
                    .take_field(output.clone())
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;

                let typed_val = output_val
                    .downcast::<#arg_type>()
                    .map_err(|_| directed::InjectionError::OutputTypeMismatch(output.clone()))?;

                node.set_input_changed(true);

                // TODO: Do something with old val
                node.inputs.#arg_name.replace((*typed_val, directed::ReevaluationRule::Move));
            }
        });

        inject_clone_arms.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                // For transparent parent, clone the output
                let output_val = parent.outputs_mut()
                    .field_mut(output.clone())
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;

                let typed_val = output_val
                    .downcast_ref::<#arg_type>()
                    .ok_or_else(|| directed::InjectionError::OutputTypeMismatch(output.clone()))?;
                
                // Check if changed
                let input_changed = if let Some((existing_val, _)) = &node.inputs.#arg_name {
                    *typed_val != *existing_val
                } else {
                    true
                };
                
                if input_changed && !node.input_changed() {
                    node.set_input_changed(true);
                }
                
                // Set on concrete node
                // TODO: Do something with old val
                node.inputs.#arg_name.replace((typed_val.clone(), directed::ReevaluationRule::CacheLast));
            }
        });
    }

    Ok(quote_spanned! {Span::call_site()=>

        #[allow(unreachable_code)]
        fn inject_move(
            node: &mut directed::Node<#stage_name>,
            parent: &mut Box<dyn directed::AnyNode>,
            output: Option<&'static directed::TypeReflection>,
            input: Option<&'static directed::TypeReflection>
        ) -> Result<(), directed::InjectionError> {
            match input.map(|input| input.name) {
                #(#inject_move_arms)*
                _ => return Err(directed::InjectionError::InputNotFound(input))
            }
            Ok(())
        }

        #[allow(unreachable_code)]
        fn inject_clone(
            node: &mut directed::Node<#stage_name>,
            parent: &mut Box<dyn directed::AnyNode>,
            output: Option<&'static directed::TypeReflection>,
            input: Option<&'static directed::TypeReflection>
        ) -> Result<(), directed::InjectionError> {
            match input.map(|input| input.name) {
                #(#inject_clone_arms)*
                _ => return Err(directed::InjectionError::InputNotFound(input))
            }
            Ok(())
        }
    })
}

fn generate_output_handling(
    config: &StageConfig,
    output_struct_name: &syn::Ident,
    cache_strategy: (CacheStrategy, Span),
) -> Result<proc_macro2::TokenStream, Diagnostic> {
    let stage_name = &config.stage_name;
    let arg_names = config
        .inputs
        .iter()
        .map(|input| &input.clean_name)
        .collect::<Vec<_>>();
    let state_as_inputs = config
        .states
        .iter()
        .map(|(name, _ty, span)| {
            quote_spanned! {*span=> &mut state.#name}
        })
        .collect::<Vec<_>>();
    let await_call = if config.is_async.0 {
        quote::quote! {.await}
    } else {
        quote::quote! {}
    };
    let fn_call = match &config.outputs {
        OutputParams::Explicit(_output_params) => {
            quote_spanned! {Span::mixed_site()=> {
                    #stage_name::call(#(#state_as_inputs),* #(#arg_names),*) #await_call
                }
            }
        }
        OutputParams::Implicit(_, span) => {
            quote_spanned! {*span=>
                #output_struct_name(Some(#stage_name::call(#(#state_as_inputs),* #(#arg_names),*) #await_call))
            }
        }
    };

    if cache_strategy.0 == CacheStrategy::All {
        Ok(quote_spanned! {cache_strategy.1=>

            // Use a hasher
            let hash: u64 = {
                #[allow(unused_imports)]
                use std::hash::Hash;
                #[allow(unused_imports)]
                use std::hash::Hasher;
                #[allow(unused_mut)]
                let mut hasher = std::hash::DefaultHasher::new();
                #(#arg_names.hash(&mut hasher);)*
                hasher.finish()
            };

            // Check cache
            #[allow(unused_variables)]
            let cached = cache.get(&hash).and_then(|cached_vec| {
                cached_vec.iter().find(|cached| {
                    cached.inputs == *inputs
                })
            });

            if let Some(cached) = cached {
                // Just use cached values
                Ok(cached.outputs.clone())
            } else {
                // Call and store result in cache
                let result = #fn_call;

                let cache_entry = directed::Cached {
                    inputs: inputs.clone(),
                    outputs: result.clone(),
                };

                cache.entry(hash).or_insert_with(Vec::new).push(cache_entry);
                Ok(result)
            }
        })
    } else {
        // No advanced caching, just run it
        Ok(quote_spanned!(cache_strategy.1=>Ok(#fn_call)))
    }
}

fn prepare_input_types(config: &StageConfig) -> Result<Vec<proc_macro2::TokenStream>, Diagnostic> {
    let args = config
        .inputs
        .iter()
        .map(|input| (&input.clean_name, &input.ty, &input.ref_type));
    let mut output = Vec::new();
    for (arg_name, ty, ref_type) in args {
        let reeval_name = quote::format_ident!("{}_reevaluation_rule", arg_name);
        match ref_type {
            RefType::Owned => {
                output.push(quote_spanned!{arg_name.span()=>
                    let #arg_name: #ty = match #reeval_name {
                        directed::ReevaluationRule::Move => {
                            // Parent is opaque
                            #arg_name
                        },
                        directed::ReevaluationRule::CacheLast | directed::ReevaluationRule::CacheAll => {
                            // Parent is transparent, clone the value
                            #arg_name.clone()
                        },
                    };
                });
            }
            RefType::Borrowed => {
                output.push(quote_spanned! {arg_name.span()=>
                    let #arg_name = &#arg_name;
                });
            }
            RefType::BorrowedMut => {
                output.push(quote_spanned! {arg_name.span()=>
                    let #arg_name = &mut #arg_name;
                });
            },
        }
    }
    Ok(output)
}

fn generate_dyn_fields_impl<P: Param>(params: impl Iterator<Item = P>) -> proc_macro2::TokenStream {
    let mut field_arms: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut field_mut_arms: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut take_field_arms: Vec<proc_macro2::TokenStream> = Vec::new();
    for param in params {
        let (name, _, _span) = param.param();
        match &name {
            syn::Member::Named(ident) => {
                let name_str = ident.to_string();
                field_arms.push(quote_spanned! {Span::mixed_site()=>
                    Some(#name_str) => self.#name.as_ref().map(|s| s as &dyn std::any::Any),
                });
                field_mut_arms.push(quote_spanned! {Span::mixed_site()=>
                    Some(#name_str) => self.#name.as_mut().map(|s| s as &mut dyn std::any::Any),
                });
                take_field_arms.push(quote_spanned! {Span::mixed_site()=>
                    Some(#name_str) => self.#name.take().map(|a| Box::new(a) as Box<dyn std::any::Any>),
                });
            }
            syn::Member::Unnamed(index) => {
                field_arms.push(quote_spanned! {Span::mixed_site()=>
                    None => self.#index.as_ref().map(|s| s as &dyn std::any::Any),
                });
                field_mut_arms.push(quote_spanned! {Span::mixed_site()=>
                    None => self.#index.as_mut().map(|s| s as &mut dyn std::any::Any),
                });
                take_field_arms.push(quote_spanned! {Span::mixed_site()=>
                    None => self.#index.take().map(|a| Box::new(a) as Box<dyn std::any::Any>),
                });
            }
        }
    }
    quote_spanned! {Span::mixed_site()=>
       fn field<'a>(&'a self, field: Option<&'static directed::TypeReflection>) -> Option<&'a (dyn std::any::Any + 'static)> {
            match field.map(|field| field.name) {
                #(#field_arms)*
                _ => None
            }
        }

        fn field_mut<'a>(&'a mut self, field: Option<&'static directed::TypeReflection>) -> Option<&'a mut (dyn std::any::Any + 'static)> {
            match field.map(|field| field.name) {
                #(#field_mut_arms)*
                _ => None
            }
        }

        fn take_field(&mut self, field: Option<&'static directed::TypeReflection>) -> Option<Box<dyn std::any::Any>> {
            match field.map(|field| field.name) {
                #(#take_field_arms)*
                _ => None
            }
        }
    }
}

/// Modify the function to give it access to concrete output type
fn insert_local_types(config: &mut StageConfig, output_type: proc_macro2::TokenStream) {
    let original_fn = &mut config.original_fn;
    let original_body = &mut original_fn.block;

    original_body.stmts.insert(
        0,
        syn::Stmt::Item(syn::Item::Type(syn::ItemType {
            attrs: Vec::new(),
            vis: syn::Visibility::Inherited,
            type_token: Token![type](Span::call_site()),
            ident: syn::Ident::new("StageOutputType", Span::call_site()),
            generics: syn::Generics::default(),
            eq_token: Token![=](Span::call_site()),
            ty: Box::new(syn::Type::Verbatim(output_type)),
            semi_token: Token![;](Span::call_site()),
        })),
    );
}

/// The core trait that defines a stage - the culmination of this macro
fn generate_stage_impl(mut config: StageConfig) -> Result<proc_macro2::TokenStream, Diagnostic> {
    // Generate names for types
    let stage_name_str = config.stage_name.to_string();
    let input_struct_name = quote::format_ident! {"{}InputCache", &config.stage_name};
    let output_struct_name = quote::format_ident! {"{}OutputCache", &config.stage_name};
    let state_struct_name = quote::format_ident! {"{}State", &config.stage_name};

    // Modify stage before starting the generation
    insert_local_types(&mut config, quote::quote! {#output_struct_name});

    let original_fn = &config.original_fn;
    let stage_name = &config.stage_name;
    let states = &config.states;
    let fn_attrs = &original_fn.attrs;
    let fn_vis = &original_fn.vis;
    let original_args = &original_fn.sig.inputs;
    let fn_return_type = &original_fn.sig.output;
    let original_body = &original_fn.block;
    let async_status = &original_fn.sig.asyncness;

    // Determine derives for inputs
    let input_derives = match config.cache_strategy.0 {
        CacheStrategy::None => {
            quote_spanned! {config.cache_strategy.1=>#[derive(Default)]}
        }
        CacheStrategy::Last => {
            quote_spanned! {config.cache_strategy.1=>#[derive(Default, Clone, PartialEq)]}
        }
        CacheStrategy::All => {
            quote_spanned! {config.cache_strategy.1=>#[derive(Default, Clone, PartialEq, Eq, Hash)]}
        }
    };
    // Determine derives for outputs
    let output_derives = match config.cache_strategy.0 {
        CacheStrategy::None => quote::quote! {#[derive(Default)]},
        CacheStrategy::Last | CacheStrategy::All => {
            quote_spanned! {config.cache_strategy.1=>#[derive(Default, Clone)]}
        }
    };

    // Create useful structs
    let input_struct_fields =
        proc_macro2::TokenStream::from_iter(config.inputs.iter().map(|input| {
            let input_ident = &input.clean_name;
            let input_ty = input.unwrapped_type();
            quote_spanned! {input.span=>
                #input_ident: Option<(#input_ty, directed::ReevaluationRule)>,
            }
        }));
    let input_struct = quote_spanned! {Span::call_site()=>
        #input_derives
        #fn_vis struct #input_struct_name {
            #input_struct_fields
        }
    };
    let input_dyn_fields_trait_impl = generate_dyn_fields_impl(config.inputs.iter());
    let input_shape = {
        let input_fields = config.inputs.iter().map(|field| {
            let name = field.clean_name.to_string();
            let ty = &field.ty;
            let ty_string = quote::quote!{#ty}.to_string();
            quote::quote!{
                directed::TypeReflection { name: #name, ty: #ty_string }
            }
        });
        quote::quote!{
            impl #input_struct_name {
                const SHAPE: &'static [directed::TypeReflection] = &[#(#input_fields),*];
            }
        }
    };

    let output_struct = match &config.outputs {
        OutputParams::Explicit(output_params) => {
            let mut fields = Vec::new();
            for param in output_params.iter() {
                let name = &param.name;
                let ty = &param.ty;
                let span = param.span;

                fields.push(quote_spanned! {span=> #name: Option<#ty>});
            }
            quote_spanned! {Span::mixed_site()=>
                #output_derives
                #fn_vis struct #output_struct_name {
                    #(#fields),*
                }
            }
        }
        OutputParams::Implicit(ty, span) => quote_spanned! {*span=>
            #output_derives
            #fn_vis struct #output_struct_name(Option<#ty>);
        },
    };

    let output_dyn_fields_trait_impl = match &config.outputs {
        OutputParams::Explicit(output_params) => generate_dyn_fields_impl(output_params.iter()),
        OutputParams::Implicit(_, span) => generate_dyn_fields_impl(std::iter::once(TupleParam {
            idx: syn::Index::from(0),
            ty: syn::TypeNever {
                bang_token: syn::Token![!](*span),
            }
            .into(),
            span: Span::mixed_site(),
        })),
    };
    let output_shape = {
        let output_fields = match &config.outputs {
            OutputParams::Explicit(output_params) => output_params.iter().map(|field| {
                let name = field.name.to_string();
                let ty = &field.ty;
                let ty_string = quote::quote!{#ty}.to_string();
                quote::quote_spanned!{field.span=>
                    directed::TypeReflection { name: #name, ty: #ty_string }
                }
            }).collect(),
            OutputParams::Implicit(ty, span) => {
                let ty_string = quote::quote!{#ty}.to_string();
                vec!(quote::quote_spanned!{*span=>
                    directed::TypeReflection { name: "_", ty: #ty_string }
                })
            },
        };
        quote::quote!{
            impl #output_struct_name {
                const SHAPE: &'static [directed::TypeReflection] = &[#(#output_fields),*];
            }
        }
    };

    let state_struct_fields =
        proc_macro2::TokenStream::from_iter(states.iter().map(|(state_ident, ty, span)| {
            let state_ty = &ty;
            quote_spanned! {*span=>
                #state_ident: #state_ty,
            }
        }));
    // If state is a unit, implement default
    let default_derive = if config.states.is_empty() {
        quote_spanned! { Span::call_site()=> #[derive(Default)] }
    } else {
        quote::quote!{}
    };
    let state_struct = quote_spanned! {Span::call_site()=>
        #default_derive
        #fn_vis struct #state_struct_name {
            #state_struct_fields
        }
    };

    // Generate code sections
    let extraction_code = generate_extraction_code(&config.inputs, config.cache_strategy)?;
    let injection_code = input_injection(stage_name, &config.inputs)?;
    let prepare_input_types_code = prepare_input_types(&config)?;
    let output_handling =
        generate_output_handling(&config, &output_struct_name, config.cache_strategy)?;

    // Determine evaluation strategy and reevaluation rule
    let eval_strategy = if config.is_lazy.0 {
        quote_spanned! {config.is_lazy.1=> directed::EvalStrategy::Lazy }
    } else {
        quote_spanned! {config.is_lazy.1=> directed::EvalStrategy::Urgent }
    };

    let reevaluation_rule = match &config.cache_strategy {
        (CacheStrategy::None, span) => quote_spanned! {*span=> directed::ReevaluationRule::Move },
        (CacheStrategy::Last, span) => {
            quote_spanned! {*span=> directed::ReevaluationRule::CacheLast }
        }
        (CacheStrategy::All, span) => {
            quote_spanned! {*span=> directed::ReevaluationRule::CacheAll }
        }
    };

    // Generate function inputs
    let state_as_input = states
        .iter()
        .map(|(name, ty, span)| quote_spanned! {*span=>#name: &mut #ty});

    // Redefine the original function
    let return_type = match &config.outputs {
        OutputParams::Explicit(_) => quote::quote! {-> #output_struct_name},
        OutputParams::Implicit(_, span) => quote::quote_spanned! {*span => #fn_return_type},
    };
    let call_fn_def = quote::quote! {
        #(#fn_attrs)*
        #async_status fn call(#(#state_as_input,)* #original_args) #return_type #original_body
    };

    // Slap together eval logic
    let eval_logic = quote_spanned! {Span::call_site()=>
        #(#extraction_code)*
        #(#prepare_input_types_code)*
        #output_handling
    };
    let sync_eval_logic = if async_status.is_some() {
        quote::quote!(panic!("Attempted to call async code synchronously"))
    } else {
        quote::quote!(#eval_logic)
    };
    let async_eval_logic = if async_status.is_some() {
        quote::quote!(#eval_logic)
    } else {
        quote::quote!(#eval_logic)
    };

    let async_trait_derive = if cfg!(feature = "tokio") {
        quote::quote!{#[async_trait::async_trait]}
    } else {
        quote::quote!{}
    };

    let evaluate_impls = if cfg!(feature = "tokio") {
        quote::quote!{
            async fn evaluate_async(
                &self,
                state: &mut Self::State,
                inputs: &mut Self::Input,
                cache: &mut std::collections::HashMap<u64, Vec<directed::Cached<Self>>>,
            ) -> Result<Self::Output, InjectionError> {
                #async_eval_logic
            }

            fn evaluate(
                &self,
                state: &mut Self::State,
                inputs: &mut Self::Input,
                cache: &mut std::collections::HashMap<u64, Vec<directed::Cached<Self>>>,
            ) -> Result<Self::Output, InjectionError> {
                #sync_eval_logic
            }
        }
    } else {
        quote::quote!{
            fn evaluate(
                &self,
                state: &mut Self::State,
                inputs: &mut Self::Input,
                cache: &mut std::collections::HashMap<u64, Vec<directed::Cached<Self>>>,
            ) -> Result<Self::Output, InjectionError> {
                #sync_eval_logic
            }
        }
    };

    // The coup de grace
    Ok(quote_spanned! {Span::call_site()=>
        // Create a struct implementing the Stage trait
        #[derive(Debug, Clone, Copy, Default)]
        #fn_vis struct #stage_name;

        impl #stage_name {
            /// Evaluate this stage
            #call_fn_def
        }

        // Structs to contain various caches
        #input_struct
        #input_shape
        #output_struct
        #output_shape
        #state_struct

        impl directed::DynFields for #output_struct_name {
            #output_dyn_fields_trait_impl

            fn replace(&mut self, other: Box<dyn std::any::Any>) -> Box<dyn DynFields> {
                Box::new(std::mem::replace::<Self>(self, *other.downcast::<Self>().expect("DynFields type must be exact match")))
            }
        }

        impl directed::DynFields for #input_struct_name {
            #input_dyn_fields_trait_impl

            fn replace(&mut self, other: Box<dyn std::any::Any>) -> Box<dyn DynFields> {
                Box::new(std::mem::replace::<Self>(self, *other.downcast::<Self>().expect("DynFields type must be exact match")))
            }
        }

        #async_trait_derive
        impl directed::Stage for #stage_name {
            const SHAPE: directed::StageShape = directed::StageShape {
                stage_name: #stage_name_str,
                inputs: Self::Input::SHAPE,
                outputs: Self::Output::SHAPE,
            };
            type Input = #input_struct_name;
            type Output = #output_struct_name;
            type State = #state_struct_name;

            #evaluate_impls

            fn eval_strategy(&self) -> directed::EvalStrategy {
                #eval_strategy
            }

            fn reeval_rule(&self) -> directed::ReevaluationRule {
                #reevaluation_rule
            }

            fn inject_input(&self, node: &mut directed::Node<Self>, parent: &mut Box<dyn directed::AnyNode>, output: Option<&'static directed::TypeReflection>, input: Option<&'static directed::TypeReflection>) -> Result<(), directed::InjectionError> {
                #injection_code
            }
        }
    })
}
