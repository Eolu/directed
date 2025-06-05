mod parse;

use proc_macro::TokenStream;
use proc_macro_error::proc_macro_error;
use proc_macro2::Span;
use quote::quote_spanned;
use syn::{
    parse::{Parse, ParseStream}, parse_macro_input, punctuated::Punctuated, spanned::Spanned, FnArg, ItemFn, Pat, ReturnType, Token, Type, TypeGenerics
};
use proc_macro2_diagnostics::{SpanDiagnosticExt, Diagnostic};
use parse::*;

// TODO: `inject_input` implementation is unruly and violates DRY in silly ways. It could be simplified a lot.

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
        Err(diag) => diag.emit_as_expr_tokens().into()
    }
}

/// This associates the names of function parameters with the TypeId of their type.
///
/// Used to build and validate I/O-based connections
fn generate_input_registrations(inputs: &[InputParam]) -> Result<Vec<proc_macro2::TokenStream>, Diagnostic> {
    Ok(inputs.iter().map(|input| {
        let arg_name = &input.clean_name;
        let arg_type = &input.ty;
        let ref_type = input.ref_type.quoted();
        let span = input.span.clone();
        quote_spanned! {span=>
            inputs.insert(directed::DataLabel::new_with_type_name(#arg_name, stringify!(#arg_type)), (std::any::TypeId::of::<#arg_type>(), #ref_type));
        }
    }).collect())
}

/// This associates the names of function outputs with the TypeId of their type.
/// When a function returns named outputs, this will associate meaningful
/// names to each output. When a function returns any other type, this will
/// simply associate that one type with the name '_'.
///
/// Used to build and validate I/O-based connections
fn generate_output_registrations(
    outputs: &OutputParams,
) -> Result<Vec<proc_macro2::TokenStream>, Diagnostic> {
    // TODO: DataLabel stuff here should be subsumed by better arch
    match outputs {
        OutputParams::Explicit(outputs) => {
            // TODO: I don't even think this will work anymore with the struct!!
            Ok(outputs.iter().map(|output| {
                let name = output.name.to_string();
                let ty = &output.ty;
                let span = &output.span;
                quote_spanned! {*span=>
                    // TODO: Fix the fact it can't find outputs here!
                    outputs.insert(directed::DataLabel::new_with_type_name(#name, stringify!(#ty)), std::any::TypeId::of::<#ty>());
                }
            }).collect())
        },
        OutputParams::Implicit(ty, span) => {
            Ok(vec![quote_spanned! {*span=>
                // TODO: Fix the fact it can't find outputs here!
                outputs.insert(directed::DataLabel::new_with_type_name("_", stringify!(#ty)), std::any::TypeId::of::<#ty>());
            }])
        },
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
        let arg_name = &input.name;
        let arg_type = true_type(&input.ty);
        let clean_arg_name = &input.clean_name;
        let reeval_name = quote::format_ident!("{}_reevaluation_rule", clean_arg_name);
        let input_span = input.span.clone();

        match cache_strategy {
            (CacheStrategy::None, _span) => quote_spanned! {input_span=>
                // Non-transparent functions never clone, always move
                let #reeval_name: inputs.#arg_name.1;
                let #arg_name: std::sync::Arc<#arg_type> = if #reeval_name == directed::ReevaluationRule::Move {
                    if let Some((input, _)) = inputs.#arg_name.take() {
                        input
                    } else {
                        return Err(directed::InjectionError::InputNotFound(directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))));
                    }
                } else {
                    if let Some((input, _)) = &inputs.#arg_name {
                        input.clone()
                    } else {
                        return Err(directed::InjectionError::InputNotFound(directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))));
                    }
                };
            },
            (CacheStrategy::Last, _span) | (CacheStrategy::All, _span) => quote_spanned! {input_span=>
                let (#arg_name, #reeval_name): (std::sync::Arc<#arg_type>, directed::ReevaluationRule) = if let Some((input, reeval_rule)) = &inputs.#arg_name {
                    (input.clone(), *reeval_rule)
                } else {
                    return Err(directed::InjectionError::InputNotFound(directed::DataLabel::new_with_type_name(#clean_arg_name, stringify!(#arg_type))));
                };
            },
        }
    }).collect())
}

/// This generates the code that uses the output of a parent node to set the
/// input of a child node.
fn input_injection(inputs: &[InputParam]) -> Result<proc_macro2::TokenStream, Diagnostic> {
    let mut match_arms = Vec::new();

    // Build match arms from inputs
    for input in inputs.iter() {
        let clean_arg_name = &input.clean_name;
        let span = input.span.clone();

        match_arms.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                // Cast node to the concrete type
                let node = node.as_any_mut()
                    .downcast_mut::<directed::Node<Self>>()
                    .ok_or_else(|| directed::InjectionError::InputTypeMismatch(input.0.clone()))?;
                
                // Get output from parent (still type-erased)
                let parent_any = parent.as_any();
                
                // We need to handle different cases based on parent's reevaluation rule
                if parent.reeval_rule() == directed::ReevaluationRule::Move {
                    // Parent is opaque - take the output
                    if node.reeval_rule() == directed::ReevaluationRule::Move && input.1 != directed::RefType::Owned {
                        // Both are opaque but child takes reference - share the Arc
                        inject_transparent_out_to_opaque_ref_in(node, parent, output, &input.0)?;
                    } else {
                        // Take ownership from parent
                        inject_opaque_out(node, parent, output, &input.0)?;
                    }
                } else {
                    // Parent is transparent - clone the output
                    inject_transparent_out_to_owned_in(node, parent, output, &input.0)?;
                }
                
                Ok(())
            }
        });
    }

    // Add the default case
    let default_case = quote_spanned! {Span::call_site()=>
        Some(name) => Err(directed::InjectionError::InputNotFound(name.into())),
        None => Ok(()) // This means there's a connection with no data associated
    };
    match_arms.push(default_case);

    // Generate the helper functions that work with concrete types
    let inject_helpers = generate_inject_helpers(&inputs)?;

    Ok(quote_spanned! {Span::call_site()=>
        #inject_helpers
        
        match input.0.inner() {
            #(#match_arms)*
        }
    })
}

fn generate_inject_helpers(inputs: &[InputParam]) -> Result<proc_macro2::TokenStream, Diagnostic> {
    let mut inject_opaque_out_arms = Vec::new();
    let mut inject_transparent_out_to_owned_in_arms = Vec::new();
    let mut inject_transparent_out_to_opaque_ref_in_arms = Vec::new();

    for input in inputs.iter() {
        let arg_name = &input.name;
        let clean_arg_name = &input.clean_name;
        let arg_type = true_type(&input.ty);
        let span = input.span.clone();

        inject_opaque_out_arms.push(quote_spanned! {span=>
            #clean_arg_name => {
                // For opaque parent, we need to remove the output
                // This is tricky since parent is type-erased - we need a way to remove outputs generically
                // For now, assume parent has a method to take outputs
                let output_val = parent.take_output(&output)
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;
                
                let output_val = output_val
                    .downcast::<#arg_type>()
                    .map_err(|_| directed::InjectionError::OutputTypeMismatch(output.clone()))?;
                
                // Set on concrete node
                if node.inputs.is_none() {
                    node.inputs = Some(Default::default());
                }
                if let Some(inputs) = &mut node.inputs {
                    inputs.#arg_name = Some((output_val, directed::ReevaluationRule::Move));
                }
                node.set_input_changed(true);
            }
        });

        inject_transparent_out_to_owned_in_arms.push(quote_spanned! {span=>
            #clean_arg_name => {
                // For transparent parent, clone the output
                let output_val = parent.get_output(&output)
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;
                
                let typed_val = output_val
                    .downcast_ref::<#arg_type>()
                    .ok_or_else(|| directed::InjectionError::OutputTypeMismatch(output.clone()))?;
                
                // Check if changed
                let input_changed = if let Some(inputs) = &node.inputs {
                    if let Some((existing_val, _)) = &inputs.#arg_name {
                        existing_val.as_ref() != typed_val
                    } else {
                        true
                    }
                } else {
                    true
                };
                
                if input_changed && !node.input_changed() {
                    node.set_input_changed(true);
                }
                
                // Set on concrete node
                if node.inputs.is_none() {
                    node.inputs = Some(Default::default());
                }
                if let Some(inputs) = &mut node.inputs {
                    inputs.#arg_name = Some((std::sync::Arc::new(typed_val.clone()), directed::ReevaluationRule::CacheLast));
                }
            }
        });

        inject_transparent_out_to_opaque_ref_in_arms.push(quote_spanned! {span=>
            #clean_arg_name => {
                // Share the Arc directly
                let output_arc = parent.get_output_arc(&output)
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;
                
                let typed_arc = std::sync::Arc::downcast::<#arg_type>(output_arc)
                    .map_err(|_| directed::InjectionError::OutputTypeMismatch(output.clone()))?;
                
                // Check if changed
                let input_changed = if let Some(inputs) = &node.inputs {
                    if let Some((existing_val, _)) = &inputs.#arg_name {
                        !std::sync::Arc::ptr_eq(existing_val, &typed_arc)
                    } else {
                        true
                    }
                } else {
                    true
                };
                
                if input_changed && !node.input_changed() {
                    node.set_input_changed(true);
                }
                
                // Set on concrete node
                if node.inputs.is_none() {
                    node.inputs = Some(Default::default());
                }
                if let Some(inputs) = &mut node.inputs {
                    inputs.#arg_name = Some((typed_arc, directed::ReevaluationRule::CacheLast));
                }
            }
        });
    }

    Ok(quote_spanned! {Span::call_site()=>
        fn inject_opaque_out(
            node: &mut directed::Node<Self>,
            parent: &mut Box<dyn directed::AnyNode>,
            output: directed::DataLabel,
            input_name: &str
        ) -> Result<(), directed::InjectionError> {
            match input_name {
                #(#inject_opaque_out_arms)*
                _ => Err(directed::InjectionError::InputNotFound(input_name.into()))
            }
            Ok(())
        }

        fn inject_transparent_out_to_owned_in(
            node: &mut directed::Node<Self>,
            parent: &mut Box<dyn directed::AnyNode>,
            output: directed::DataLabel,
            input_name: &str
        ) -> Result<(), directed::InjectionError> {
            match input_name {
                #(#inject_transparent_out_to_owned_in_arms)*
                _ => Err(directed::InjectionError::InputNotFound(input_name.into()))
            }
            Ok(())
        }

        fn inject_transparent_out_to_opaque_ref_in(
            node: &mut directed::Node<Self>,
            parent: &mut Box<dyn directed::AnyNode>,
            output: directed::DataLabel,
            input_name: &str
        ) -> Result<(), directed::InjectionError> {
            match input_name {
                #(#inject_transparent_out_to_opaque_ref_in_arms)*
                _ => Err(directed::InjectionError::InputNotFound(input_name.into()))
            }
            Ok(())
        }
    })
}

fn generate_output_handling(
    config: &StageConfig,
    cache_strategy: (CacheStrategy, Span),
    input_struct_name: syn::Ident,
    output_struct_name: syn::Ident,
) -> Result<proc_macro2::TokenStream, Diagnostic> {
    // TODO: Right now cache_last and cache_all are handled more differently than necessary
    let stage_name = &config.stage_name;
    let arg_names = config
        .inputs
        .iter()
        .map(|input| &input.name)
        .collect::<Vec<_>>();
    let clean_names = config
        .inputs
        .iter()
        .map(|input| &input.clean_name)
        .collect::<Vec<_>>();
    let arg_types = config
        .inputs
        .iter()
        .map(|input| &input.ty)
        .collect::<Vec<_>>();
    let state_as_inputs = match config.states.len(){
        0 => vec![],
        1 => {
            vec![quote_spanned!{config.states[0].2=> state}]
        },
        _ => config.states.iter()
                .enumerate()
                .map(|(i, state)| {
                    let idx = syn::Index::from(i);
                    quote_spanned!{state.2=> &mut state.#idx}
                })
                .collect::<Vec<_>>()
    };
    // TODO: Get all output types annotated
    // let first_output_type = config
    //     .outputs
    //     .iter()
    //     .map(|(_, t, _)| t)
    //     .cloned()
    //     .next()
    //     .unwrap_or(Type::Tuple(syn::TypeTuple {
    //         paren_token: syn::token::Paren::default(),
    //         elems: Punctuated::new(),
    //     }));
    let await_call = if config.is_async.0 { quote::quote!{.await} } else { quote::quote!{} };
    let fn_call = match &config.outputs {
        OutputParams::Explicit(_) => {
            quote_spanned! {Span::mixed_site()=> #stage_name::call(#(#state_as_inputs),* #(#arg_names),*) #await_call }
        },
        OutputParams::Implicit(_, span) => quote_spanned! {*span=> #output_struct_name(#stage_name::call(#(#state_as_inputs),* #(#arg_names),*) #await_call) },
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

            // Check equality
            #[allow(unused_variables)]
            let cached = cache.get(&hash).and_then(|cached| {
                #[allow(unused_imports)]
                use directed::DowncastEq;
                cached.iter().find(|cached| cached.inputs == *inputs)
            });

            if let Some(cached) = cached {
                // Just use cached values
                Ok(cached.outputs.clone())
            } else {
                // Call and store result in cache
                let result = #fn_call;
                let cache_entry = {
                    #[allow(unused_mut)]
                    let mut cached = directed::Cached::<#input_struct_name, #output_struct_name> {
                        inputs: inputs.clone(),
                        outputs: result.clone(),
                    };
                    cached
                };
                if let None = cache.get(&hash) {
                    cache.insert(hash, Vec::new());
                }
                if let Some(vec) = cache.get_mut(&hash) {
                    vec.push(cache_entry);
                }
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
        .map(|input| (&input.name, &input.clean_name, &input.ref_type));
    let mut output = Vec::new();
    for (arg_name, clean_name, ref_type) in args {
        let reeval_name = quote::format_ident!("{}_reevaluation_rule", clean_name);
        match ref_type {
            RefType::Owned => {
                output.push(quote_spanned!{arg_name.span()=>
                    let #arg_name = match #reeval_name {
                        directed::ReevaluationRule::Move => {
                            // Parent is opaque, use Arc::into_inner
                            match std::sync::Arc::into_inner(#arg_name) {
                                Some(arg) => arg,
                                None => {return Err(directed::InjectionError::TooManyReferences(stringify!(#arg_name)))}
                            }
                        },
                        directed::ReevaluationRule::CacheLast | directed::ReevaluationRule::CacheAll => {
                            // Parent is transparent, clone the value
                            (*#arg_name).clone()
                        },
                    };
                });
            }
            RefType::Borrowed => {
                output.push(quote_spanned! {arg_name.span()=>
                    // TODO: if node is transparent (config.cache_strategy != None), error with a graceful message (rather than letting clone fail)
                    let #arg_name = #arg_name.as_ref();
                });
            }
            RefType::BorrowedMut => panic!("Mutable refs are not yet supported"),
        }
    }
    Ok(output)
}

/// Modify the function to give it access to concrete types
/// TODO: Is this necessary?
fn insert_local_types(config: &mut StageConfig, output_type: proc_macro2::TokenStream) {
    let original_fn = &mut config.original_fn;
    let original_body = &mut original_fn.block;

    original_body.stmts.insert(0, syn::Stmt::Item(syn::Item::Type(syn::ItemType {
        attrs: Vec::new(),
        vis: syn::Visibility::Inherited,
        type_token: Token![type](Span::call_site()),
        ident: syn::Ident::new("StageOutputType", Span::call_site()),
        generics:  syn::Generics::default(),
        eq_token: Token![=](Span::call_site()),
        ty: Box::new(syn::Type::Verbatim(output_type)),
        semi_token: Token![;](Span::call_site()),
    })));
}

/// The core trait that defines a stage - the culmination of this macro
fn generate_stage_impl(mut config: StageConfig) -> Result<proc_macro2::TokenStream, Diagnostic> {

    // Generate names for types
    let input_struct_name = quote::format_ident!{"{}InputCache", &config.stage_name};
    let output_struct_name = quote::format_ident!{"{}OutputCache", &config.stage_name};
    let state_struct_name = quote::format_ident!{"{}State", &config.stage_name};

    // Modify stage before starting the generation
    insert_local_types(&mut config, quote::quote!{#output_struct_name});

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
        CacheStrategy::None => quote::quote!{},
        CacheStrategy::Last => quote_spanned!{config.cache_strategy.1=>#[derive(Clone, PartialEq)]},
        CacheStrategy::All => quote_spanned!{config.cache_strategy.1=>#[derive(Clone, PartialEq, Eq, Hash)]},
    };
    // Determine derives for outputs
    let output_derives = match config.cache_strategy.0 {
        CacheStrategy::None => quote::quote!{},
        CacheStrategy::Last | CacheStrategy::All => quote_spanned!{config.cache_strategy.1=>#[derive(Clone)]},
    };

    // Create useful structs
    let input_struct_fields = proc_macro2::TokenStream::from_iter(config.inputs.iter().map(|input| {
        let input_ident = syn::Ident::new(&input.clean_name, input.span);
        let input_ty = input.unwrapped_type();
        quote_spanned!{input.span=>
            #input_ident: Option<(std::sync::Arc<#input_ty>, directed::ReevaluationRule)>,
        }
    }));
    let input_struct = quote_spanned!{Span::call_site()=>
        #input_derives
        #fn_vis struct #input_struct_name {
            #input_struct_fields
        }
    };

    let output_struct = match &config.outputs {
        OutputParams::Explicit(output_params) => {
            let fields = output_params.iter().map(|param| {
                let name = &param.name;
                let ty = &param.ty;
                let span = param.span;

                quote_spanned! {span=> #name: #ty}
            });
            quote_spanned! {Span::mixed_site()=>
                #output_derives
                #fn_vis struct #output_struct_name {
                    #(#fields),*
                }
            }
        },
        OutputParams::Implicit(ty, span) => quote_spanned! {*span=>
            #output_derives
            #fn_vis struct #output_struct_name(#ty);
        },
    };

    let state_struct_fields = proc_macro2::TokenStream::from_iter(config.states.iter().map(|(state_ident, ty, span)| {
        let state_ty = &ty;
        quote_spanned!{*span=>
            #state_ident: #state_ty,
        }
    }));
    let state_struct = quote_spanned!{Span::call_site()=> 
        #fn_vis struct #state_struct_name {
            #state_struct_fields
        }
    };

    // Generate code sections
    let extraction_code = generate_extraction_code(&config.inputs, config.cache_strategy)?;
    let injection_code = input_injection(&config.inputs)?;
    let prepare_input_types_code = prepare_input_types(&config)?;
    let output_handling = generate_output_handling(&config, config.cache_strategy, input_struct_name.clone(), output_struct_name.clone())?;

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
    let state_as_input = states.iter().map(|(name,ty,span)| quote_spanned! {*span=>#name: &mut #ty});
    let state_as_type = match states.len() {
        0 => quote::quote!{()},
        1 => {
            let (_,ty,span) = &states[0];
            quote_spanned! {*span=>#ty}
        },
        _ => {
            let types = states.iter().map(|(_,ty,span)| quote_spanned! {*span=>#ty});
            quote::quote!{(#(#types),*,)}
        }
    };

    // Redefine the original function
    let return_type = match &config.outputs {
        OutputParams::Explicit(_) => quote::quote!{-> #output_struct_name},
        OutputParams::Implicit(_, span) => quote::quote_spanned!{*span => #fn_return_type},
    };
    let call_fn_def = quote::quote! {
        #(#fn_attrs)*
        #async_status fn call(#(#state_as_input),* #original_args) #return_type #original_body
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

    // The coup de grace
    Ok(quote_spanned! {Span::call_site()=>
        // Create a struct implementing the Stage trait
        #[derive(Debug, Clone, Copy)]
        #fn_vis struct #stage_name;

        impl #stage_name {
            /// Evaluate this stage
            #call_fn_def
        }

        // Structs to contain various caches
        #input_struct
        #output_struct
        #state_struct

        #[cfg_attr(feature = "tokio", async_trait::async_trait)]
        impl directed::Stage for #stage_name {
            type Input = #input_struct_name;
            type Output = #output_struct_name;
            type State = #state_struct_name;
            type StateStruct = #state_struct_name;

            fn inputs(&self) -> &std::collections::HashMap<directed::DataLabel, (std::any::TypeId, directed::RefType)> {
                // TODO
                unimplemented!()
            }

            fn outputs(&self) -> &std::collections::HashMap<directed::DataLabel, std::any::TypeId> {
                // TODO
                unimplemented!()
            }

            fn evaluate(
                &self,
                state: &mut Self::State,
                inputs: &mut Self::Input,
                cache: &mut std::collections::HashMap<u64, Vec<directed::Cached<#input_struct_name, #output_struct_name>>>
            ) -> Result<Self::Output, directed::InjectionError> {
                #sync_eval_logic
            }

            #[cfg(feature = "tokio")]
            async fn evaluate_async(
                &self,
                state: &mut Self::State,
                inputs: &mut Self::Input,
                cache: &mut std::collections::HashMap<u64, Vec<directed::Cached<#input_struct_name, #output_struct_name>>>
            ) -> Result<Self::Output, directed::InjectionError> {
                #async_eval_logic
            }

            fn eval_strategy(&self) -> directed::EvalStrategy {
                #eval_strategy
            }

            fn reeval_rule(&self) -> directed::ReevaluationRule {
                #reevaluation_rule
            }

            fn inject_input(&self, node: &mut directed::Node<Self>, parent: &mut Box<dyn directed::AnyNode>) -> Result<(), directed::InjectionError> {
                #injection_code
            }

            fn name(&self) -> &str {
                stringify!(#stage_name)
            }
        }
    })
}
