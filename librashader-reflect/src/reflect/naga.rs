use crate::error::ShaderReflectError;
use crate::front::naga::NagaCompilation;
use crate::front::shaderc::GlslangCompilation;
use naga::front::spv::Options;
use naga::Module;

#[derive(Debug)]
pub struct NagaReflect {
    vertex: Module,
    fragment: Module,
}

impl TryFrom<NagaCompilation> for NagaReflect {
    type Error = ShaderReflectError;

    fn try_from(value: NagaCompilation) -> Result<Self, Self::Error> {
        Ok(NagaReflect {
            vertex: value.vertex,
            fragment: value.fragment,
        })
    }
}

impl TryFrom<GlslangCompilation> for NagaReflect {
    type Error = ShaderReflectError;

    fn try_from(value: GlslangCompilation) -> Result<Self, Self::Error> {
        let ops = Options::default();
        let vertex = naga::front::spv::parse_u8_slice(value.vertex.as_binary_u8(), &ops)?;
        let fragment = naga::front::spv::parse_u8_slice(value.fragment.as_binary_u8(), &ops)?;
        Ok(NagaReflect { vertex, fragment })
    }
}

#[cfg(test)]
mod test {
    use crate::reflect::naga::NagaReflect;
    use naga::front::spv::Options;

    #[test]
    pub fn test_into() {
        let result = librashader_preprocess::load_shader_source(
            "../test/slang-shaders/blurs/shaders/royale/blur3x3-last-pass.slang",
        )
        .unwrap();
        let spirv = crate::front::shaderc::compile_spirv(&result).unwrap();

        println!("{:?}", NagaReflect::try_from(spirv))
    }
}