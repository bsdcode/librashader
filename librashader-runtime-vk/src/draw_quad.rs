use crate::error;
use crate::vulkan_primitives::VulkanBuffer;
use ash::vk;
use std::sync::Arc;

#[rustfmt::skip]
pub(crate) static VBO_DEFAULT_FINAL: &[f32; 16] = &[
    // Final
    0.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 1.0,
    1.0, 0.0, 1.0, 0.0,
    1.0, 1.0, 1.0, 1.0,
];

pub struct DrawQuad {
    buffer: VulkanBuffer,
    device: Arc<ash::Device>,
}

impl DrawQuad {
    pub fn new(
        device: &Arc<ash::Device>,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
    ) -> error::Result<DrawQuad> {
        let mut buffer = VulkanBuffer::new(
            device,
            mem_props,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            std::mem::size_of::<[f32; 16]>(),
        )?;

        {
            let mut map = buffer.map()?;
            unsafe {
                map.copy_from(0, bytemuck::cast_slice(VBO_DEFAULT_FINAL));
            }
        }
        Ok(DrawQuad {
            buffer,
            device: device.clone(),
        })
    }

    pub fn bind_vbo(&self, cmd: vk::CommandBuffer) {
        unsafe {
            self.device
                .cmd_bind_vertex_buffers(cmd, 0, &[self.buffer.handle], &[0])
        }
    }
}
