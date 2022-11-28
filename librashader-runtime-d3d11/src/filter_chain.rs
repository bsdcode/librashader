use crate::texture::OwnedTexture;
use librashader_common::image::Image;
use librashader_common::Size;
use librashader_preprocess::ShaderSource;
use librashader_presets::{ShaderPassConfig, ShaderPreset, TextureConfig};
use librashader_reflect::back::cross::GlslangHlslContext;
use librashader_reflect::back::targets::HLSL;
use librashader_reflect::back::{CompilerBackend, CompileShader, FromCompilation};
use librashader_reflect::front::shaderc::GlslangCompilation;
use librashader_reflect::reflect::semantics::{ReflectSemantics, SemanticMap, TextureSemantics, UniformBinding, UniformSemantic, VariableSemantics};
use librashader_reflect::reflect::ReflectShader;
use rustc_hash::FxHashMap;
use std::error::Error;
use std::path::Path;
use bytemuck::offset_of;
use windows::core::PCSTR;
use windows::s;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_WRITE, D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_RESOURCE_MISC_FLAG, D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SAMPLER_DESC, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC, ID3D11Buffer, ID3D11Device, ID3D11DeviceContext};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
use crate::filter_pass::{ConstantBuffer, ConstantBufferBinding, FilterPass};
use crate::samplers::SamplerSet;
use crate::util;
use crate::util::d3d11_compile_bound_shader;

type ShaderPassMeta<'a> = (
    &'a ShaderPassConfig,
    ShaderSource,
    CompilerBackend<
        impl CompileShader<HLSL, Options = Option<()>, Context = GlslangHlslContext> + ReflectShader,
    >,
);

#[repr(C)]
#[derive(Default)]
struct D3D11VertexLayout {
    position: [f32; 2],
    texcoord: [f32; 2],
    color: [f32; 4],
}

pub struct FilterChain {
    pub common: FilterCommon,
    pub passes: Vec<FilterPass>,
}

pub struct Direct3D11 {
    pub(crate) device: ID3D11Device,
    pub(crate) device_context: ID3D11DeviceContext,
}

pub struct FilterCommon {
    pub(crate) d3d11: Direct3D11,
    pub(crate) preset: ShaderPreset,
    pub(crate) luts: FxHashMap<usize, OwnedTexture>,
    pub samplers: SamplerSet,
}

impl FilterChain {
    fn load_pass_semantics(
        uniform_semantics: &mut FxHashMap<String, UniformSemantic>,
        texture_semantics: &mut FxHashMap<String, SemanticMap<TextureSemantics>>,
        config: &ShaderPassConfig,
    ) {
        let Some(alias) = &config.alias else {
            return;
        };

        // Ignore empty aliases
        if alias.trim().is_empty() {
            return;
        }

        let index = config.id as usize;

        // PassOutput
        texture_semantics.insert(
            alias.clone(),
            SemanticMap {
                semantics: TextureSemantics::PassOutput,
                index,
            },
        );
        uniform_semantics.insert(
            format!("{alias}Size"),
            UniformSemantic::Texture(SemanticMap {
                semantics: TextureSemantics::PassOutput,
                index,
            }),
        );

        // PassFeedback
        texture_semantics.insert(
            format!("{alias}Feedback"),
            SemanticMap {
                semantics: TextureSemantics::PassFeedback,
                index,
            },
        );
        uniform_semantics.insert(
            format!("{alias}FeedbackSize"),
            UniformSemantic::Texture(SemanticMap {
                semantics: TextureSemantics::PassFeedback,
                index,
            }),
        );
    }

    fn create_constant_buffer(device: &ID3D11Device, size: u32) -> util::Result<ID3D11Buffer> {
        eprintln!("{size}");
        unsafe {
           let buffer = device.CreateBuffer(&D3D11_BUFFER_DESC {
                ByteWidth: size,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE,
                MiscFlags: D3D11_RESOURCE_MISC_FLAG(0),
                StructureByteStride: 0,
            }, None)?;

            Ok(buffer)
        }
    }

    fn init_passes(
        device: &ID3D11Device,
        passes: Vec<ShaderPassMeta>,
        semantics: &ReflectSemantics,
    ) -> util::Result<Vec<FilterPass>>
    {
        // let mut filters = Vec::new();
        let mut filters = Vec::new();

        for (index, (config, source, mut reflect)) in passes.into_iter().enumerate() {
            let reflection = reflect.reflect(index, semantics)?;
            let hlsl = reflect.compile(None)?;

            let vertex_dxil = util::d3d_compile_shader(
                hlsl.vertex.as_bytes(),
                b"main\0",
                b"vs_5_0\0"
            )?;
            let vs = d3d11_compile_bound_shader(device, &vertex_dxil, None,
                                                ID3D11Device::CreateVertexShader)?;

            let ia_desc = [
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: PCSTR(b"TEXCOORD\0".as_ptr()),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: offset_of!(D3D11VertexLayout, position) as u32,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: PCSTR(b"TEXCOORD\0".as_ptr()),
                    SemanticIndex: 1,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: offset_of!(D3D11VertexLayout, texcoord) as u32,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                }
            ];
            let vertex_ia = util::d3d11_create_input_layout(device, &ia_desc, &vertex_dxil)?;

            let fragment_dxil = util::d3d_compile_shader(
                hlsl.fragment.as_bytes(),
                b"main\0",
                b"ps_5_0\0"
            )?;
            let ps = d3d11_compile_bound_shader(device, &fragment_dxil, None,
                                                ID3D11Device::CreatePixelShader)?;


            let ubo_cbuffer = if let Some(ubo) = &reflection.ubo && ubo.size != 0 {
                let buffer = FilterChain::create_constant_buffer(device, ubo.size)?;
                Some(ConstantBufferBinding {
                    binding: ubo.binding,
                    size: ubo.size,
                    stage_mask: ubo.stage_mask,
                    buffer,
                })
            } else {
                None
            };

            let push_cbuffer = if let Some(push) = &reflection.push_constant && push.size != 0 {
                let buffer = FilterChain::create_constant_buffer(device, push.size)?;
                Some(ConstantBufferBinding {
                    binding: if ubo_cbuffer.is_some() { 1 } else { 0 },
                    size: push.size,
                    stage_mask: push.stage_mask,
                    buffer,
                })
            } else {
                None
            };

            let mut uniform_bindings = FxHashMap::default();
            for param in reflection.meta.parameter_meta.values() {
                uniform_bindings.insert(
                    UniformBinding::Parameter(param.id.clone()),
                    param.offset,
                );
            }

            for (semantics, param) in &reflection.meta.variable_meta {
                uniform_bindings.insert(
                    UniformBinding::SemanticVariable(*semantics),
                    param.offset
                );
            }

            for (semantics, param) in &reflection.meta.texture_size_meta {
                uniform_bindings.insert(
                    UniformBinding::TextureSize(*semantics),
                    param.offset
                );
            }

            filters.push(FilterPass {
                reflection,
                compiled: hlsl,
                vertex_shader: vs,
                vertex_layout: vertex_ia,
                pixel_shader: ps,
                uniform_bindings,
                uniform_buffer: ConstantBuffer::new(ubo_cbuffer),
                push_buffer: ConstantBuffer::new(push_cbuffer),
                source,
                config: config.clone(),
            })

        }
        Ok(filters)
    }
    /// Load a filter chain from a pre-parsed `ShaderPreset`.
    pub fn load_from_preset(device: &ID3D11Device, preset: ShaderPreset) -> util::Result<FilterChain> {
        let (passes, semantics) = FilterChain::load_preset(&preset)?;

        let samplers = SamplerSet::new(device)?;

        // initialize passes
        let filters = FilterChain::init_passes(device, passes, &semantics).unwrap();

        // let default_filter = filters.first().map(|f| f.config.filter).unwrap_or_default();
        // let default_wrap = filters
        //     .first()
        //     .map(|f| f.config.wrap_mode)
        //     .unwrap_or_default();

        // // initialize output framebuffers
        // let mut output_framebuffers = Vec::new();
        // output_framebuffers.resize_with(filters.len(), || Framebuffer::new(1));
        // let mut output_textures = Vec::new();
        // output_textures.resize_with(filters.len(), Texture::default);
        //
        // // initialize feedback framebuffers
        // let mut feedback_framebuffers = Vec::new();
        // feedback_framebuffers.resize_with(filters.len(), || Framebuffer::new(1));
        // let mut feedback_textures = Vec::new();
        // feedback_textures.resize_with(filters.len(), Texture::default);

        // load luts
        let luts = FilterChain::load_luts(device, &preset.textures)?;

        // let (history_framebuffers, history_textures) =
        //     FilterChain::init_history(&filters, default_filter, default_wrap);

        let mut device_context = None;

        unsafe {
            device.GetImmediateContext(&mut device_context);
        }

        // todo: make vbo: d3d11.c 1376
        Ok(FilterChain {
            passes: filters,
            // output_framebuffers: output_framebuffers.into_boxed_slice(),
            // feedback_framebuffers: feedback_framebuffers.into_boxed_slice(),
            // history_framebuffers,
            // filter_vao,
            common: FilterCommon {
                d3d11: Direct3D11 {
                    device: device.clone(),
                    device_context: device_context.unwrap()
                },
                luts,
                samplers,
                // we don't need the reflect semantics once all locations have been bound per pass.
                // semantics,
                preset,
                // output_textures: output_textures.into_boxed_slice(),
                // feedback_textures: feedback_textures.into_boxed_slice(),
                // history_textures,
                // draw_quad,
            },
        })
    }

    fn load_luts(
        device: &ID3D11Device,
        textures: &[TextureConfig],
    ) -> util::Result<FxHashMap<usize, OwnedTexture>> {
        let mut luts = FxHashMap::default();

        for (index, texture) in textures.iter().enumerate() {
            let image = Image::load(&texture.path)?;
            let desc = D3D11_TEXTURE2D_DESC {
                Width: image.size.width,
                Height: image.size.height,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                Usage: D3D11_USAGE_DEFAULT,
                MiscFlags: if texture.mipmap {
                    D3D11_RESOURCE_MISC_GENERATE_MIPS
                } else {
                    D3D11_RESOURCE_MISC_FLAG(0)
                },
                ..Default::default()
            };

            let mut texture = OwnedTexture::new(device, &image, desc,
                                                texture.filter_mode, texture.wrap_mode)?;
            luts.insert(index, texture);
        }
        Ok(luts)
    }

    /// Load the shader preset at the given path into a filter chain.
    pub fn load_from_path(device: &ID3D11Device, path: impl AsRef<Path>) -> util::Result<FilterChain> {
        // load passes from preset
        let preset = ShaderPreset::try_parse(path)?;
        Self::load_from_preset(device, preset)
    }

    fn load_preset(preset: &ShaderPreset) -> util::Result<(Vec<ShaderPassMeta>, ReflectSemantics)> {
        let mut uniform_semantics: FxHashMap<String, UniformSemantic> = Default::default();
        let mut texture_semantics: FxHashMap<String, SemanticMap<TextureSemantics>> =
            Default::default();

        let passes = preset
            .shaders
            .iter()
            .map(|shader| {
                eprintln!("[dx11] loading {}", &shader.name.display());
                let source: ShaderSource = ShaderSource::load(&shader.name)?;

                let spirv = GlslangCompilation::compile(&source)?;
                let reflect = HLSL::from_compilation(spirv)?;

                for parameter in source.parameters.iter() {
                    uniform_semantics.insert(
                        parameter.id.clone(),
                        UniformSemantic::Variable(SemanticMap {
                            semantics: VariableSemantics::FloatParameter,
                            index: (),
                        }),
                    );
                }
                Ok::<_, Box<dyn Error>>((shader, source, reflect))
            })
            .into_iter()
            .collect::<util::Result<Vec<(&ShaderPassConfig, ShaderSource, CompilerBackend<_>)>>>()?;

        for details in &passes {
            FilterChain::load_pass_semantics(
                &mut uniform_semantics,
                &mut texture_semantics,
                details.0,
            )
        }

        // add lut params
        for (index, texture) in preset.textures.iter().enumerate() {
            texture_semantics.insert(
                texture.name.clone(),
                SemanticMap {
                    semantics: TextureSemantics::User,
                    index,
                },
            );

            uniform_semantics.insert(
                format!("{}Size", texture.name),
                UniformSemantic::Texture(SemanticMap {
                    semantics: TextureSemantics::User,
                    index,
                }),
            );
        }

        let semantics = ReflectSemantics {
            uniform_semantics,
            texture_semantics: texture_semantics,
        };

        Ok((passes, semantics))
    }
}