use crate::error;
use ash::vk;
use ash::vk::{
    AttachmentLoadOp, AttachmentStoreOp, ImageLayout, PipelineBindPoint, SampleCountFlags,
};

pub struct VulkanRenderPass {
    pub handle: vk::RenderPass,
    pub _format: vk::Format,
}

impl VulkanRenderPass {
    pub fn create_render_pass(device: &ash::Device, format: vk::Format) -> error::Result<Self> {
        // format should never be undefined.
        let attachment = [vk::AttachmentDescription::default()
            .flags(vk::AttachmentDescriptionFlags::empty())
            .format(format)
            .samples(SampleCountFlags::TYPE_1)
            .load_op(AttachmentLoadOp::DONT_CARE)
            .store_op(AttachmentStoreOp::STORE)
            .stencil_load_op(AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(AttachmentStoreOp::DONT_CARE)
            .initial_layout(ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];

        let attachment_ref = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];

        let subpass = [vk::SubpassDescription::default()
            .pipeline_bind_point(PipelineBindPoint::GRAPHICS)
            .color_attachments(&attachment_ref)];

        let renderpass_info = vk::RenderPassCreateInfo::default()
            .flags(vk::RenderPassCreateFlags::empty())
            .attachments(&attachment)
            .subpasses(&subpass);

        unsafe {
            let rp = device.create_render_pass(&renderpass_info, None)?;
            Ok(Self {
                handle: rp,
                _format: format,
            })
        }
    }
}
