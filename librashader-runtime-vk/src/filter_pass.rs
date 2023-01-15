use std::sync::Arc;
use crate::{error, VulkanImage};
use crate::filter_chain::FilterCommon;
use crate::render_target::RenderTarget;
use crate::samplers::SamplerSet;
use crate::texture::InputImage;
use crate::ubo_ring::VkUboRing;
use crate::vulkan_state::VulkanGraphicsPipeline;
use ash::vk;
use librashader_common::{ImageFormat, Size, Viewport};
use librashader_preprocess::ShaderSource;
use librashader_presets::ShaderPassConfig;
use librashader_reflect::reflect::semantics::{
    BindingStage, MemberOffset, TextureBinding, TextureSemantics, UniformBinding, UniqueSemantics,
};
use librashader_reflect::reflect::ShaderReflection;
use librashader_runtime::uniforms::{BindUniform, NoUniformBinder, UniformStorage, UniformStorageAccess};
use rustc_hash::FxHashMap;
use librashader_runtime::binding::{BindSemantics, TextureInput};

pub struct FilterPass {
    pub device: Arc<ash::Device>,
    pub reflection: ShaderReflection,
    // pub(crate) compiled: ShaderCompilerOutput<Vec<u32>>,
    pub(crate) uniform_storage: UniformStorage,
    pub uniform_bindings: FxHashMap<UniformBinding, MemberOffset>,
    pub source: ShaderSource,
    pub config: ShaderPassConfig,
    pub graphics_pipeline: VulkanGraphicsPipeline,
    pub ubo_ring: VkUboRing,
    pub frames_in_flight: u32,
}

impl TextureInput for InputImage {
    fn size(&self) -> Size<u32> {
        self.image.size
    }
}

impl BindSemantics for FilterPass {
    type InputTexture = InputImage;
    type SamplerSet = SamplerSet;
    type DescriptorSet<'a> = vk::DescriptorSet;
    type DeviceContext = Arc<ash::Device>;
    type UniformOffset = MemberOffset;

    fn bind_texture<'a>(
        descriptors: &mut Self::DescriptorSet<'a>, samplers: &Self::SamplerSet,
        binding: &TextureBinding, texture: &Self::InputTexture, device: &Self::DeviceContext) {
        let sampler = samplers.get(texture.wrap_mode, texture.filter_mode, texture.mip_filter);
        let image_info = [vk::DescriptorImageInfo::builder()
            .sampler(sampler.handle)
            .image_view(texture.image_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .build()];

        let write_desc = [vk::WriteDescriptorSet::builder()
            .dst_set(*descriptors)
            .dst_binding(binding.binding)
            .dst_array_element(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info)
            .build()];
        unsafe {
            device.update_descriptor_sets(&write_desc, &[]);
        }
    }
}

impl FilterPass {
    #[inline(always)]
    fn bind_texture(
        device: &ash::Device,
        samplers: &SamplerSet,
        descriptor_set: vk::DescriptorSet,
        binding: &TextureBinding,
        texture: &InputImage,
    ) {
        let sampler = samplers.get(texture.wrap_mode, texture.filter_mode, texture.mip_filter);
        let image_info = [vk::DescriptorImageInfo::builder()
            .sampler(sampler.handle)
            .image_view(texture.image_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .build()];

        let write_desc = [vk::WriteDescriptorSet::builder()
            .dst_set(descriptor_set)
            .dst_binding(binding.binding)
            .dst_array_element(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info)
            .build()];
        unsafe {
            device.update_descriptor_sets(&write_desc, &[]);
        }
    }

    pub fn get_format(&self) -> ImageFormat {
        let fb_format = self.source.format;
        if let Some(format) = self.config.get_format_override() {
            format
        } else if fb_format == ImageFormat::Unknown {
            ImageFormat::R8G8B8A8Unorm
        } else {
            fb_format
        }
    }

    pub(crate) fn draw(
        &mut self,
        cmd: vk::CommandBuffer,
        pass_index: usize,
        parent: &FilterCommon,
        frame_count: u32,
        frame_direction: i32,
        viewport: &Viewport<VulkanImage>,
        original: &InputImage,
        source: &InputImage,
        output: &RenderTarget,
    ) -> error::Result<()> {
        let mut descriptor = *&self.graphics_pipeline.layout.descriptor_sets
            [(frame_count % self.frames_in_flight) as usize];

        self.build_semantics(
            pass_index,
            parent,
            &output.mvp,
            frame_count,
            frame_direction,
            output.output.size,
            viewport.output.size,
            &mut descriptor,
            original,
            source,
        );

        if let Some(ubo) = &self.reflection.ubo {
            // shader_vulkan: 2554 (ra uses uses one big buffer)
            // itll be simpler for us if we just use a RingBuffer<vk::Buffer> tbh.
            self.ubo_ring
                .bind_to_descriptor_set(descriptor, ubo.binding, &self.uniform_storage)?;
        }

        output.output.begin_pass(cmd);

        let attachments = [vk::RenderingAttachmentInfo::builder()
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image_view(output.output.image_view)
            .build()];

        let rendering_info = vk::RenderingInfo::builder()
            .layer_count(1)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: output.output.size.into(),
            })
            .color_attachments(&attachments);

        unsafe {
            parent.device.cmd_begin_rendering(cmd, &rendering_info);
            parent.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.graphics_pipeline.pipeline,
            );

            // todo: allow frames in flight.
            parent.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.graphics_pipeline.layout.layout,
                0,
                &[descriptor],
                &[],
            );

            if let Some(push) = &self.reflection.push_constant {
                let mut stage_mask = vk::ShaderStageFlags::empty();
                if push.stage_mask.contains(BindingStage::FRAGMENT) {
                    stage_mask |= vk::ShaderStageFlags::FRAGMENT;
                }
                if push.stage_mask.contains(BindingStage::VERTEX) {
                    stage_mask |= vk::ShaderStageFlags::VERTEX;
                }

                parent.device.cmd_push_constants(
                    cmd,
                    self.graphics_pipeline.layout.layout,
                    stage_mask,
                    0,
                    self.uniform_storage.push_slice(),
                );
            }

            parent.draw_quad.bind_vbo(cmd);

            parent.device.cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D {
                        x: output.x as i32,
                        y: output.y as i32,
                    },
                    extent: output.output.size.into(),
                }],
            );

            parent
                .device
                .cmd_set_viewport(cmd, 0, &[output.output.size.into()]);
            parent.device.cmd_draw(cmd, 4, 1, 0, 0);
            parent.device.cmd_end_rendering(cmd);
        }
        Ok(())
    }

    fn build_semantics(
        &mut self,
        pass_index: usize,
        parent: &FilterCommon,
        mvp: &[f32; 16],
        frame_count: u32,
        frame_direction: i32,
        fb_size: Size<u32>,
        viewport_size: Size<u32>,
        mut descriptor_set: &mut vk::DescriptorSet,
        original: &InputImage,
        source: &InputImage,
    ) {
        Self::bind_semantics(
            &self.device,
            &parent.samplers,
            &mut self.uniform_storage,
            &mut descriptor_set,
            mvp,
            frame_count,
            frame_direction,
            fb_size,
            viewport_size,
            original,
            source,
            &self.uniform_bindings,
            &self.reflection.meta.texture_meta,
            parent.output_inputs[0..pass_index].iter()
                .map(|o| o.as_ref()),
            parent.feedback_inputs.iter()
                .map(|o| o.as_ref()),
            parent.history_textures.iter()
                .map(|o| o.as_ref()),
            parent.luts.iter()
                .map(|(u, i)| (*u, i.as_ref())),
            &self.source.parameters,
            &parent.config.parameters
        );
    }
}
