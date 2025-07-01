mod parse;

use parse::*;
use proc_macro::TokenStream;
use proc_macro_error::proc_macro_error;
use proc_macro2::Span;
use proc_macro2_diagnostics::{Diagnostic, SpanDiagnosticExt};
use quote::quote_spanned;
use syn::{
    FnArg, ItemFn, Pat, ReturnType, Token, Type, TypeGenerics,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    spanned::Spanned,
};

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
        Err(diag) => diag.emit_as_expr_tokens().into(),
    }
}

/// This associates the names of function parameters with the TypeId of their type.
///
/// Used to build and validate I/O-based connections
fn generate_input_registrations(
    inputs: &[InputParam],
) -> Result<Vec<proc_macro2::TokenStream>, Diagnostic> {
    Ok(inputs.iter().map(|input| {
        let arg_name = &input.clean_name;
        let arg_type = &input.ty;
        let ref_type = input.ref_type.quoted();
        let span = input.span.clone();
        quote_spanned! {span=>
            inputs.insert(directed::facet::Field::new_with_type_name(#arg_name, stringify!(#arg_type)), (std::any::TypeId::of::<#arg_type>(), #ref_type));
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
    match outputs {
        OutputParams::Explicit(outputs) => {
            Ok(outputs.iter().map(|output| {
                let name = output.name.to_string();
                let ty = &output.ty;
                let span = &output.span;
                quote_spanned! {*span=>
                    outputs.insert(directed::facet::Field::new_with_type_name(#name, stringify!(#ty)), std::any::TypeId::of::<#ty>());
                }
            }).collect())
        },
        OutputParams::Implicit(ty, span) => {
            Ok(vec![quote_spanned! {*span=>
                outputs.insert(directed::facet::Field::new_with_type_name("_", stringify!(#ty)), std::any::TypeId::of::<#ty>());
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
                let (#arg_name, #reeval_name): (std::sync::Arc<#arg_type>, directed::ReevaluationRule) = {
                    if let Some((arg, reeval_name)) = inputs.#arg_name.take() {
                        (arg, reeval_name)
                    } else {
                        // TODO: Graceful error
                        panic!("ERROR (CacheStrategy::None) NOT YET IMPLEMENTED");
                    }
                };
            },
            (CacheStrategy::Last, _span) | (CacheStrategy::All, _span) => quote_spanned! {input_span=>
                let (#arg_name, #reeval_name): (std::sync::Arc<#arg_type>, directed::ReevaluationRule) = {
                    if let Some((arg, reeval_name)) = &inputs.#arg_name {
                        (arg.clone(), *reeval_name)
                    } else {
                        // TODO: Graceful error
                        panic!("ERROR (CacheStrategy::Last | CacheStrategy::All) NOT YET IMPLEMENTED");
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
        let clean_arg_name = &input.clean_name;
        let span = input.span.clone();

        match_arms.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                // Cast node to the concrete type
                let node = node.as_any_mut()
                    .downcast_mut::<directed::Node<Self>>()
                    .ok_or_else(|| directed::InjectionError::InputTypeMismatch(input))?;
                
                // We need to handle different cases based on parent's reevaluation rule
                if parent.reeval_rule() == directed::ReevaluationRule::Move {
                    // Parent is opaque - take the output
                    if node.reeval_rule() == directed::ReevaluationRule::Move && node.stage_shape().input_fields().iter()
                            .find(|field| input.filter(|input| input.name == field.name).is_some())
                            .map(|field| directed::RefType::from(field)).unwrap_or(directed::RefType::Owned) != directed::RefType::Owned {
                        // Both are opaque but child takes reference - share the Arc
                        inject_transparent_out_to_opaque_ref_in(node, parent, output, input)?;
                    } else {
                        // Take ownership from parent
                        inject_opaque_out(node, parent, output, input)?;
                    }
                } else {
                    // Parent is transparent - clone the output
                    inject_transparent_out_to_owned_in(node, parent, output, input)?;
                }
                
                Ok(())
            }
        });
    }

    // Add the default case
    let default_case = quote_spanned! {Span::call_site()=>
        Some(name) => {
            // TODO: Verify the unwrap in unreachable
            Err(directed::InjectionError::InputNotFound(#stage_name::SHAPE.input_fields().iter().find(|field| field.name == name)))
        },
        None => Ok(()) // This means there's a connection with no data associated
    };
    match_arms.push(default_case);

    // Generate the helper functions that work with concrete types
    let inject_helpers = generate_inject_helpers(stage_name, &inputs)?;

    Ok(quote_spanned! {Span::call_site()=>
        #inject_helpers

        let result: Result<(), directed::error::InjectionError> = match input.map(|input| input.name) {
            #(#match_arms)*
        };
        result
    })
}

fn generate_inject_helpers(
    stage_name: &syn::Ident,
    inputs: &[InputParam],
) -> Result<proc_macro2::TokenStream, Diagnostic> {
    let mut inject_opaque_out_arms = Vec::new();
    let mut inject_transparent_out_to_owned_in_arms = Vec::new();
    let mut inject_transparent_out_to_opaque_ref_in_arms = Vec::new();

    for input in inputs.iter() {
        let arg_name = &input.name;
        let clean_arg_name = &input.clean_name;
        let arg_type = true_type(&input.ty);
        let span = input.span.clone();

        inject_opaque_out_arms.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                // For opaque parent, we need to remove the output
                let output_val = parent.outputs_mut()
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?
                    .take_field(output.clone())
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;

                let typed_val = output_val
                    .downcast::<#arg_type>()
                    .ok_or_else(|| directed::InjectionError::OutputTypeMismatch(output.clone()))?;

                node.set_input_changed(true);

                // TODO: Do something with old val
                node.inputs.#arg_name.replace((typed_val, directed::ReevaluationRule::Move));
            }
        });

        inject_transparent_out_to_owned_in_arms.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                // For transparent parent, clone the output
                let output_val = parent.outputs_mut()
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?
                    .field_mut(output.clone())
                    .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?;

                let typed_val = output_val
                    .downcast_ref::<#arg_type>()
                    .ok_or_else(|| directed::InjectionError::OutputTypeMismatch(output.clone()))?;
                
                // Check if changed
                let input_changed = if let Some((existing_val, _)) = &node.inputs.#arg_name {
                    *typed_val != **existing_val
                } else {
                    true
                };
                
                if input_changed && !node.input_changed() {
                    node.set_input_changed(true);
                }
                
                // Set on concrete node
                // TODO: Do something with old val
                node.inputs.#arg_name.replace((std::sync::Arc::new(typed_val.clone()), directed::ReevaluationRule::CacheLast));
            }
        });

        inject_transparent_out_to_opaque_ref_in_arms.push(quote_spanned! {span=>
            Some(#clean_arg_name) => {
                unimplemented!("inject_transparent_out_to_opaque_ref_in_arms");
                // Share the Arc directly
                // let output_arc = parent.outputs_mut()
                //     .get(&output)
                //     .ok_or_else(|| directed::InjectionError::OutputNotFound(output.clone()))?
                //     .clone();

                // // Verify the type
                // if !output_arc.is::<#arg_type>() {
                //     return Err(directed::InjectionError::OutputTypeMismatch(output.clone()));
                // }

                // // Check if changed
                // let input_changed = if let Some(inputs) = &node.inputs {
                //     if let Some((existing_val, _)) = &inputs.#arg_name {
                //         !std::sync::Arc::ptr_eq(existing_val, &output_arc)
                //     } else {
                //         true
                //     }
                // } else {
                //     true
                // };

                // if input_changed && !node.input_changed() {
                //     node.set_input_changed(true);
                // }

                // // Set on concrete node
                // node.inputs = Some(output_arc);
            }
        });
    }

    Ok(quote_spanned! {Span::call_site()=>
        fn inject_opaque_out(
            node: &mut directed::Node<#stage_name>,
            parent: &mut Box<dyn directed::AnyNode>,
            output: Option<&'static directed::facet::Field>,
            input: Option<&'static directed::facet::Field>
        ) -> Result<(), directed::InjectionError> {
            match input.map(|input| input.name) {
                #(#inject_transparent_out_to_owned_in_arms)*
                _ => return Err(directed::InjectionError::InputNotFound(input))
            }
            Ok(())
        }

        fn inject_transparent_out_to_owned_in(
            node: &mut directed::Node<#stage_name>,
            parent: &mut Box<dyn directed::AnyNode>,
            output: Option<&'static directed::facet::Field>,
            input: Option<&'static directed::facet::Field>
        ) -> Result<(), directed::InjectionError> {
            match input.map(|input| input.name) {
                #(#inject_transparent_out_to_owned_in_arms)*
                _ => return Err(directed::InjectionError::InputNotFound(input))
            }
            Ok(())
        }

        fn inject_transparent_out_to_opaque_ref_in(
            node: &mut directed::Node<#stage_name>,
            parent: &mut Box<dyn directed::AnyNode>,
            output: Option<&'static directed::facet::Field>,
            input: Option<&'static directed::facet::Field>
        ) -> Result<(), directed::InjectionError> {
            unimplemented!("inject_transparent_out_to_opaque_ref_in");
            // match input.name.as_ref().map(|n| n.as_ref()) {
            //     #(#inject_transparent_out_to_opaque_ref_in_arms)*
            //     _ => return Err(directed::InjectionError::InputNotFound(input.clone()))
            // }
            // Ok(())
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
        .map(|input| &input.name)
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
            // For implicit outputs, wrap in NodeOutput::Standard
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
                    inputs.iter().all(|(k, v)| {
                        cached.inputs.get(k).map(|cached_v| {
                            std::sync::Arc::ptr_eq(&v.0, cached_v)
                        }).unwrap_or(false)
                    })
                })
            });

            if let Some(cached) = cached {
                // Just use cached values
                Ok(match cached.outputs.len() {
                    1 if cached.outputs.contains_key(&directed::node::UNNAMED_OUTPUT_FIELD) => {
                        directed::NodeOutput::Standard(cached.outputs.get(&directed::node::UNNAMED_OUTPUT_FIELD).unwrap().clone())
                    }
                    _ => directed::NodeOutput::Named(cached.outputs.clone())
                })
            } else {
                // Call and store result in cache
                let result = #fn_call;
                let outputs_map = match &result {
                    directed::NodeOutput::Standard(val) => {
                        let mut map = std::collections::HashMap::new();
                        map.insert(directed::node::UNNAMED_OUTPUT_FIELD.clone(), val.clone());
                        map
                    }
                    directed::NodeOutput::Named(map) => map.clone(),
                };

                let cache_entry = directed::Cached {
                    inputs: inputs.iter().map(|(k, (v, _))| (k.clone(), v.clone())).collect(),
                    outputs: outputs_map,
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
                    let #arg_name = #arg_name.as_ref();
                });
            }
            RefType::BorrowedMut => panic!("Mutable refs are not yet supported"),
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
       fn field<'a>(&'a self, field: Option<&'static directed::facet::Field>) -> Option<&'a (dyn std::any::Any + 'static)> {
            match field.map(|field| field.name) {
                #(#field_arms)*
                _ => None
            }
        }

        fn field_mut<'a>(&'a mut self, field: Option<&'static directed::facet::Field>) -> Option<&'a mut (dyn std::any::Any + 'static)> {
            match field.map(|field| field.name) {
                #(#field_mut_arms)*
                _ => None
            }
        }

        fn take_field(&mut self, field: Option<&'static directed::facet::Field>) -> Option<Box<dyn std::any::Any>> {
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
            quote_spanned! {config.cache_strategy.1=>#[derive(Default, directed::facet::Facet)]}
        }
        CacheStrategy::Last => {
            quote_spanned! {config.cache_strategy.1=>#[derive(Default, Clone, PartialEq, directed::facet::Facet)]}
        }
        CacheStrategy::All => {
            quote_spanned! {config.cache_strategy.1=>#[derive(Default, Clone, PartialEq, Eq, Hash, directed::facet::Facet)]}
        }
    };
    // Determine derives for outputs
    let output_derives = match config.cache_strategy.0 {
        CacheStrategy::None => quote::quote! {#[derive(Default, directed::facet::Facet)]},
        CacheStrategy::Last | CacheStrategy::All => {
            quote_spanned! {config.cache_strategy.1=>#[derive(Default, Clone, directed::facet::Facet)]}
        }
    };

    // Create useful structs
    let input_struct_fields =
        proc_macro2::TokenStream::from_iter(config.inputs.iter().map(|input| {
            let input_ident = syn::Ident::new(&input.clean_name, input.span);
            let input_ty = input.unwrapped_type();
            quote_spanned! {input.span=>
                #input_ident: Option<(std::sync::Arc<#input_ty>, directed::ReevaluationRule)>,
            }
        }));
    let input_struct = quote_spanned! {Span::call_site()=>
        #input_derives
        #fn_vis struct #input_struct_name {
            #input_struct_fields
        }
    };
    let input_dyn_fields_trait_impl = generate_dyn_fields_impl(config.inputs.iter());

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

    let set_val_output_fn = match &config.outputs {
        OutputParams::Explicit(output_params) => {
            let fields = output_params.iter().map(|param| {
                let name = &param.name;
                let name_str = name.to_string();
                let ty = &param.ty;
                let span = param.span;

                quote_spanned! {Span::mixed_site()=>
                    #name_str => Box::new(self.#name.replace(*val.downcast::<#ty>().unwrap())) as Box<dyn std::any::Any>
                }
            });
            quote_spanned! {Span::mixed_site()=> fn set_val(&mut self, name: &str, val: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
                match name {
                    #(#fields,)*
                    unknown => unreachable!("Unknown output field: {}", unknown)
                }
            }}
        }
        OutputParams::Implicit(ty, span) => {
            quote_spanned! {Span::mixed_site()=> fn set_val(&mut self, _name: &str, val: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
                Box::new(self.0.replace(*val.downcast::<#ty>().unwrap())) as Box<dyn std::any::Any>
            }}
        }
    };

    let state_struct_fields =
        proc_macro2::TokenStream::from_iter(states.iter().map(|(state_ident, ty, span)| {
            let state_ty = &ty;
            quote_spanned! {*span=>
                #state_ident: #state_ty,
            }
        }));
    let state_struct = quote_spanned! {Span::call_site()=>
        // TODO: Default derive is unnnecessarily limiting here, need to temporarily 'tuple-fy' the state
        #[derive(Default, crate::facet::Facet)]
        #fn_vis struct #state_struct_name {
            #state_struct_fields
        }
    };

    // Generate code sections
    let input_registrations = generate_input_registrations(&config.inputs)?;
    let output_registrations = generate_output_registrations(&config.outputs)?;
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
        #[derive(Debug, Clone, Copy, Default)]
        #fn_vis struct #stage_name;

        impl #stage_name {
            /// Evaluate this stage
            #call_fn_def
        }

        // Structs to contain various caches
        #input_struct
        #output_struct
        #state_struct

        impl directed::SetVal for #output_struct_name {
            #set_val_output_fn
        }

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

        #[cfg_attr(feature = "tokio", async_trait::async_trait)]
        impl directed::Stage for #stage_name {
            const SHAPE: directed::StageShape = directed::StageShape {
                stage_name: #stage_name_str,
                inputs: Self::Input::SHAPE,
                outputs: Self::Output::SHAPE,
                state: Self::State::SHAPE,
            };
            type Input = #input_struct_name;
            type Output = #output_struct_name;
            type State = #state_struct_name;

            fn evaluate(
                &self,
                state: &mut Self::State,
                inputs: &mut Self::Input,
                cache: &mut std::collections::HashMap<u64, Vec<crate::Cached<Self>>>,
            ) -> Result<Self::Output, InjectionError> {
                #sync_eval_logic
            }

            #[cfg(feature = "tokio")]
            async fn evaluate_async(
                &self,
                state: &mut Self::State,
                inputs: &mut Self::Input,
                cache: &mut std::collections::HashMap<u64, Vec<crate::Cached>>,
            ) -> Result<Self::Output, InjectionError> {
                #async_eval_logic
            }

            fn eval_strategy(&self) -> directed::EvalStrategy {
                #eval_strategy
            }

            fn reeval_rule(&self) -> directed::ReevaluationRule {
                #reevaluation_rule
            }

            fn inject_input(&self, node: &mut directed::Node<Self>, parent: &mut Box<dyn directed::AnyNode>, output: Option<&'static directed::facet::Field>, input: Option<&'static directed::facet::Field>) -> Result<(), directed::InjectionError> {
                #injection_code
            }
        }
    })
}
