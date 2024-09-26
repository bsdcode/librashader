use crate::render::RenderTest;
use anyhow::anyhow;
use image::RgbaImage;
use librashader::runtime::mtl::{FilterChain, FilterChainOptions};
use librashader::runtime::Viewport;
use librashader_runtime::image::{Image, PixelFormat, UVDirection, BGRA8, RGBA8};
use objc2::ffi::NSUInteger;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLOrigin,
    MTLPixelFormat, MTLRegion, MTLSize, MTLStorageMode, MTLTexture, MTLTextureDescriptor,
    MTLTextureUsage,
};
use std::path::Path;
use std::ptr::NonNull;

pub struct Metal {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
    image_bytes: Image<BGRA8>,
}

impl RenderTest for Metal {
    fn new(path: &Path) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        Metal::new(path)
    }

    fn render(&mut self, path: &Path, frame_count: usize) -> anyhow::Result<RgbaImage> {
        let queue = self
            .device
            .newCommandQueue()
            .ok_or_else(|| anyhow!("Unable to create command queue"))?;

        let cmd = queue
            .commandBuffer()
            .ok_or_else(|| anyhow!("Unable to create command buffer"))?;

        let mut filter_chain = FilterChain::load_from_path(
            path,
            &queue,
            Some(&FilterChainOptions {
                force_no_mipmaps: false,
            }),
        )?;

        let render_texture = unsafe {
            let texture_descriptor =
                MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                    MTLPixelFormat::BGRA8Unorm,
                    self.image_bytes.size.width as NSUInteger,
                    self.image_bytes.size.height as NSUInteger,
                    false,
                );

            texture_descriptor.setSampleCount(1);
            texture_descriptor.setStorageMode(
                if cfg!(all(target_arch = "aarch64", target_vendor = "apple")) {
                    MTLStorageMode::Shared
                } else {
                    MTLStorageMode::Managed
                },
            );
            texture_descriptor.setUsage(MTLTextureUsage::ShaderWrite);

            let texture = self
                .device
                .newTextureWithDescriptor(&texture_descriptor)
                .ok_or_else(|| anyhow!("Failed to create texture"))?;

            texture
        };

        filter_chain.frame(
            &self.texture,
            &Viewport::new_render_target_sized_origin(render_texture.as_ref(), None)?,
            cmd.as_ref(),
            frame_count,
            None,
        )?;

        cmd.commit();
        unsafe {
            cmd.waitUntilCompleted();
        }

        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: self.image_bytes.size.width as usize,
                height: self.image_bytes.size.height as usize,
                depth: 1,
            },
        };

        unsafe {
            // should be the same size
            let mut buffer = vec![0u8; self.image_bytes.bytes.len()];
            render_texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::new(buffer.as_mut_ptr().cast()).unwrap(),
                4 * self.image_bytes.size.width as usize,
                region,
                0,
            );

            // swap the BGRA back to RGBA.
            BGRA8::convert(&mut buffer);

            let image = RgbaImage::from_raw(
                render_texture.width() as u32,
                render_texture.height() as u32,
                Vec::from(buffer),
            )
            .ok_or(anyhow!("Unable to create image from data"))?;

            Ok(image)
        }
    }
}

impl Metal {
    pub fn new(image_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let image: Image<BGRA8> = Image::load(image_path, UVDirection::TopLeft)?;

        unsafe {
            let device = Retained::from_raw(MTLCreateSystemDefaultDevice())
                .ok_or_else(|| anyhow!("Unable to create default Metal device"))?;

            let texture_descriptor =
                MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                    MTLPixelFormat::BGRA8Unorm,
                    image.size.width as NSUInteger,
                    image.size.height as NSUInteger,
                    false,
                );

            texture_descriptor.setSampleCount(1);
            texture_descriptor.setStorageMode(
                if cfg!(all(target_arch = "aarch64", target_vendor = "apple")) {
                    MTLStorageMode::Shared
                } else {
                    MTLStorageMode::Managed
                },
            );
            texture_descriptor.setUsage(MTLTextureUsage::ShaderRead);

            let texture = device
                .newTextureWithDescriptor(&texture_descriptor)
                .ok_or_else(|| anyhow!("Failed to create texture"))?;

            let region = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: image.size.width as usize,
                    height: image.size.height as usize,
                    depth: 1,
                },
            };
            texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                region,
                0,
                // SAFETY: replaceRegion withBytes is const.
                NonNull::new_unchecked(image.bytes.as_slice().as_ptr() as *mut _),
                4 * image.size.width as usize,
            );

            Ok(Self {
                device,
                texture,
                image_bytes: image,
            })
        }
    }
}