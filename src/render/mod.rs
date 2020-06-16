use crate::cache::{AlignedBufferCache, Atlas, BufferCache, CacheCommon};
use bytemuck::{Pod, Zeroable};
use std::{convert::TryFrom, iter, mem, num::NonZeroU8, ops::Range};

mod pipelines;

#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    pub vertical_fov: cgmath::Deg<f32>,
    pub position: cgmath::Vector3<f32>,
    pub pitch: cgmath::Deg<f32>,
    pub yaw: cgmath::Deg<f32>,
    pub aspect_ratio: f32,
}

impl Camera {
    pub fn new(vertical_fov: impl Into<cgmath::Deg<f32>>, width: u32, height: u32) -> Self {
        Camera {
            vertical_fov: vertical_fov.into(),
            position: cgmath::vec3(0., 0., 0.),
            pitch: cgmath::Deg(0.),
            yaw: cgmath::Deg(0.),
            aspect_ratio: width as f32 / height as f32,
        }
    }

    pub fn update_pitch<F>(&mut self, func: F) -> cgmath::Deg<f32>
    where
        F: FnOnce(cgmath::Deg<f32>) -> cgmath::Deg<f32>,
    {
        self.pitch = cgmath::Deg(func(self.pitch).0.min(90.).max(-90.));
        self.pitch
    }

    pub fn update_yaw<F>(&mut self, func: F) -> cgmath::Deg<f32>
    where
        F: FnOnce(cgmath::Deg<f32>) -> cgmath::Deg<f32>,
    {
        self.yaw = cgmath::Deg(func(self.yaw).0 % 360.);
        self.yaw
    }

    pub fn set_aspect_ratio(&mut self, width: u32, height: u32) {
        self.aspect_ratio = width as f32 / height as f32;
    }

    pub fn translation(&self) -> cgmath::Matrix4<f32> {
        use cgmath::Matrix4;
        Matrix4::from_translation(-self.position)
    }

    pub fn projection(&self) -> cgmath::Matrix4<f32> {
        use cgmath::Matrix4;

        #[cfg_attr(rustfmt, rustfmt_skip)]
        const QUAKE_TO_OPENGL_TRANSFORMATION_MATRIX: Matrix4<f32> = Matrix4::new(
            0.0,0.0,  -1.0, 0.0,
            -1.0, 0.0, 0.0, 0.0,
            0.0, 1.0,  0.0, 0.0,
            0.0, 0.0,  0.0, 1.0,
        );

        let view = QUAKE_TO_OPENGL_TRANSFORMATION_MATRIX
            * Matrix4::from_angle_y(-self.pitch)
            * Matrix4::from_angle_z(-self.yaw);
        let projection = cgmath::perspective(self.vertical_fov, self.aspect_ratio, 1., 4096.);
        OPENGL_TO_WGPU_MATRIX * projection * view
    }

    pub fn matrix(&self) -> cgmath::Matrix4<f32> {
        self.projection() * self.translation()
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct ModelData {
    pub translation: cgmath::Matrix4<f32>,
    pub origin: cgmath::Vector3<f32>,
}

unsafe impl Pod for ModelData {}
unsafe impl Zeroable for ModelData {}

pub struct RenderCache {
    pub diffuse: Atlas,
    pub lightmap: Atlas,

    pub normal_vertices: BufferCache<NormalVertex>,
    pub textured_vertices: BufferCache<TexturedVertex>,
    pub world_vertices: BufferCache<WorldVertex>,

    pub indices: BufferCache<u32>,

    pub model_data: AlignedBufferCache<ModelData>,
    pub lights: BufferCache<Light>,
}

impl RenderCache {
    fn new(device: &wgpu::Device) -> Self {
        let diffuse = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tex_diffuseatlas"),
            size: DIFFUSE_ATLAS_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsage::SAMPLED | wgpu::TextureUsage::COPY_DST,
        });
        let lightmap = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tex_lightmapatlas"),
            size: LIGHTMAP_ATLAS_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsage::SAMPLED | wgpu::TextureUsage::COPY_DST,
        });

        Self {
            diffuse: Atlas::new(
                diffuse,
                DIFFUSE_ATLAS_EXTENT.width,
                DIFFUSE_ATLAS_EXTENT.height,
                1,
            ),
            lightmap: Atlas::new(
                lightmap,
                LIGHTMAP_ATLAS_EXTENT.width,
                LIGHTMAP_ATLAS_EXTENT.height,
                1,
            ),

            normal_vertices: BufferCache::new(wgpu::BufferUsage::VERTEX),
            textured_vertices: BufferCache::new(wgpu::BufferUsage::VERTEX),
            world_vertices: BufferCache::new(wgpu::BufferUsage::VERTEX),

            indices: BufferCache::new(wgpu::BufferUsage::INDEX),

            model_data: AlignedBufferCache::new(
                wgpu::BufferUsage::UNIFORM | wgpu::BufferUsage::COPY_DST,
                MINIMUM_ALIGNMENT as u16,
            ),
            lights: BufferCache::new(wgpu::BufferUsage::VERTEX),
        }
    }

    fn update(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        self.diffuse.update(device, encoder);
        self.lightmap.update(device, encoder);

        self.textured_vertices.update(device, encoder);
        self.world_vertices.update(device, encoder);
        self.normal_vertices.update(device, encoder);

        self.indices.update(device, encoder);

        self.model_data.update(device, encoder);
        self.lights.update(device, encoder);
    }
}

pub struct VertexOffset<T> {
    pub id: u64,
    _marker: std::marker::PhantomData<T>,
}

impl<T> Clone for VertexOffset<T> {
    fn clone(&self) -> Self {
        self.id.into()
    }
}

impl<T> Copy for VertexOffset<T> {}

impl<T> Default for VertexOffset<T> {
    fn default() -> Self {
        Self {
            id: Default::default(),
            _marker: Default::default(),
        }
    }
}

impl<T> From<u64> for VertexOffset<T> {
    fn from(other: u64) -> Self {
        Self {
            id: other,
            _marker: std::marker::PhantomData,
        }
    }
}

#[derive(Clone, Default)]
pub struct RenderOffsets(
    pub Option<VertexOffset<TexturedVertex>>,
    pub Option<VertexOffset<WorldVertex>>,
    pub Option<VertexOffset<NormalVertex>>,
);

impl From<VertexOffset<TexturedVertex>> for RenderOffsets {
    fn from(other: VertexOffset<TexturedVertex>) -> Self {
        Self(Some(other), None, None)
    }
}

impl From<VertexOffset<WorldVertex>> for RenderOffsets {
    fn from(other: VertexOffset<WorldVertex>) -> Self {
        Self(None, Some(other), None)
    }
}

impl From<VertexOffset<NormalVertex>> for RenderOffsets {
    fn from(other: VertexOffset<NormalVertex>) -> Self {
        Self(None, None, Some(other))
    }
}

impl From<(VertexOffset<TexturedVertex>, VertexOffset<WorldVertex>)> for RenderOffsets {
    fn from(other: (VertexOffset<TexturedVertex>, VertexOffset<WorldVertex>)) -> Self {
        Self(Some(other.0), Some(other.1), None)
    }
}

impl From<(VertexOffset<TexturedVertex>, VertexOffset<NormalVertex>)> for RenderOffsets {
    fn from(other: (VertexOffset<TexturedVertex>, VertexOffset<NormalVertex>)) -> Self {
        Self(Some(other.0), None, Some(other.1))
    }
}

#[derive(Clone)]
pub struct RenderMesh<O, I> {
    pub offsets: O,
    pub indices: I,
    pub pipeline: PipelineDesc,
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum PipelineDesc {
    Skybox,
    World,
    Models {
        origin: cgmath::Vector3<f32>,
        data_offset: wgpu::BufferAddress,
    },
}

#[derive(PartialEq, Debug, Clone, Copy)]
#[repr(C)]
pub struct Light {
    pub position: [f32; 4],
    /// R, G, B, intensity
    pub color: [f32; 4],
}

unsafe impl Pod for Light {}
unsafe impl Zeroable for Light {}

pub trait Render {
    type Indices: Iterator<Item = Range<u32>> + Clone;
    type Offsets: Into<RenderOffsets>;

    fn is_visible<T: World>(&self, _world: &T) -> bool {
        true
    }

    fn indices<T: Context>(self, ctx: &T) -> RenderMesh<Self::Offsets, Self::Indices>;
}

pub trait TransferData {
    fn transfer_data(self) -> Option<(wgpu::BufferAddress, ModelData)>;
}

pub trait World {
    type Lights: Iterator<Item = Range<u32>>;

    fn is_visible(&self, _camera: &Camera, _position: cgmath::Vector3<f32>) -> bool {
        true
    }

    fn lights(self, position: cgmath::Vector3<f32>) -> Self::Lights;
}

impl World for () {
    type Lights = iter::Empty<Range<u32>>;

    fn lights(self, _position: cgmath::Vector3<f32>) -> Self::Lights {
        iter::empty()
    }
}

struct MsaaBuffer {
    factor: NonZeroU8,
    diffuse_buffer: wgpu::TextureView,
    light_buffer: wgpu::TextureView,
}

pub struct Renderer {
    cache: RenderCache,
    matrices_buffer: wgpu::Buffer,
    fragment_uniforms: FragmentUniforms,
    fragment_uniforms_dirty: bool,
    fragment_uniforms_buffer: wgpu::Buffer,
    out_size: (u32, u32),
    nearest_sampler: wgpu::Sampler,
    linear_sampler: wgpu::Sampler,
    world_pipeline: Option<pipelines::world::Pipeline>,
    skybox_pipeline: Option<pipelines::sky::Pipeline>,
    model_pipeline: Option<pipelines::models::Pipeline>,
    rtlights_pipeline: Option<pipelines::rtlights::Pipeline>,
    post_pipeline: Option<pipelines::postprocess::Pipeline>,
    msaa_factor: NonZeroU8,
    msaa_buffer: Option<MsaaBuffer>,
    msaa_verts: wgpu::Buffer,
    depth_buffer: Option<wgpu::TextureView>,
}

// TODO: Do this better somehow
const DIFFUSE_ATLAS_EXTENT: wgpu::Extent3d = wgpu::Extent3d {
    width: 2048,
    height: 2048,
    depth: 1,
};
const LIGHTMAP_ATLAS_EXTENT: wgpu::Extent3d = wgpu::Extent3d {
    width: 2048,
    height: 2048,
    depth: 1,
};

#[derive(Debug, Copy, Clone)]
pub struct InitRendererError;

#[cfg_attr(rustfmt, rustfmt_skip)]
pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::new(
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 0.5, 0.0,
    0.0, 0.0, 0.5, 1.0,
);

#[derive(Debug, Clone, Copy)]
pub struct NormalVertex {
    pub normal: [f32; 3],
}

unsafe impl Pod for NormalVertex {}
unsafe impl Zeroable for NormalVertex {}

#[derive(Debug, Clone, Copy)]
pub struct TexturedVertex {
    pub pos: [f32; 4],
    /// For world vertices, this starts at zero and must be added to `WorldVertex::atlas_texture.xy`,
    /// for vertices of textures which don't have wrapping implemented in a shader (models and
    /// skyboxes) this is the absolute coord. We might change this to be consistent in the future,
    /// especially if we want to support wrapping everywhere.
    pub tex_coord: [f32; 2],
    /// The coordinates and size of the texture in the atlas, so we can do wrapping
    /// in the shader while still using our fake megatexture thing.
    pub atlas_texture: [u32; 4],
}

unsafe impl Pod for TexturedVertex {}
unsafe impl Zeroable for TexturedVertex {}

#[derive(Debug, Clone, Copy)]
pub struct WorldVertex {
    /// For animated textures (TODO: We can split this out even further since 99% of faces have
    /// non-animated textures)
    pub count: u32,
    pub lightmap_coord: [f32; 2],
    pub lightmap_width: f32,
    pub lightmap_count: u32,
    pub value: f32,
}

unsafe impl Pod for WorldVertex {}
unsafe impl Zeroable for WorldVertex {}

/// Get the vertices per-cluster. First element is the vertices for cluster 0, then for cluster 1, and so
/// forth.
// TODO: Maybe generate the indices-per-cluster lazily to reduce GPU memory pressure? We can recalculate
//       _only_ the indices for the cluster we're entering when we change clusters.
pub struct RenderContext<'pass, W = ()> {
    pub renderer: &'pass Renderer,
    pub camera: &'pass Camera,

    world: W,
    cur_pipeline: Option<PipelineDesc>,
    rpass: wgpu::RenderPass<'pass>,
}

pub trait Context {
    fn camera(&self) -> &Camera;
}

impl<W> Context for RenderContext<'_, W> {
    fn camera(&self) -> &Camera {
        self.camera
    }
}

impl<'pass, W> RenderContext<'pass, W>
where
    W: World + Copy,
{
    pub fn with_world<NewWorld: World>(self, world: NewWorld) -> RenderContext<'pass, NewWorld> {
        RenderContext {
            renderer: self.renderer,
            camera: self.camera,
            world,
            cur_pipeline: self.cur_pipeline,
            rpass: self.rpass,
        }
    }

    pub fn render<T>(&mut self, to_render: T)
    where
        T: Render,
    {
        // Yeah I know this is weird but it's basically free and it makes the output way
        // easier to debug in renderdoc
        #[derive(Clone)]
        struct MergeRanges<I> {
            ranges: I,
        }

        impl<I> Iterator for MergeRanges<std::iter::Peekable<I>>
        where
            I: Iterator<Item = Range<u32>>,
        {
            type Item = Range<u32>;

            fn next(&mut self) -> Option<Self::Item> {
                let mut cur = self.ranges.next()?;

                loop {
                    let next: Option<Range<u32>> = self.ranges.peek().cloned();

                    match next {
                        Some(next) if next.start == cur.end => {
                            self.ranges.next().unwrap();
                            cur = cur.start..next.end;
                        }
                        _ => return Some(cur),
                    }
                }
            }
        }

        let RenderMesh {
            offsets,
            indices,
            pipeline,
        } = to_render.indices(&*self);
        let ranges = MergeRanges {
            ranges: indices.peekable(),
        };

        let RenderOffsets(tex_o, world_o, norm_o) = offsets.into();

        let mut ranges_for_lights = None;

        if self.cur_pipeline != Some(pipeline) {
            self.cur_pipeline = Some(pipeline);

            let pipeline = match pipeline {
                PipelineDesc::Skybox => {
                    let pipeline = self.renderer.skybox_pipeline.as_ref().unwrap();

                    self.rpass.set_bind_group(0, &pipeline.bind_group, &[]);

                    &pipeline.pipeline
                }
                PipelineDesc::World => {
                    let pipeline = self.renderer.world_pipeline.as_ref().unwrap();

                    self.rpass.set_bind_group(0, &pipeline.bind_group, &[]);

                    &pipeline.pipeline
                }
                PipelineDesc::Models { data_offset, .. } => {
                    let pipeline = self.renderer.model_pipeline.as_ref().unwrap();

                    ranges_for_lights = Some(ranges.clone());

                    self.rpass.set_bind_group(
                        0,
                        &pipeline.bind_group,
                        &[u32::try_from(data_offset).unwrap()],
                    );

                    &pipeline.pipeline
                }
            };

            self.rpass.set_pipeline(&pipeline);
        }

        if let Some(verts) = &*self.renderer.cache().textured_vertices {
            self.rpass.set_vertex_buffer(
                0,
                verts.slice(tex_o.unwrap().id * mem::size_of::<TexturedVertex>() as u64..),
            );
        } else {
            return;
        }

        if let Some(world_o) = world_o {
            if let Some(verts) = &*self.renderer.cache().world_vertices {
                self.rpass.set_vertex_buffer(
                    1,
                    verts.slice(world_o.id * mem::size_of::<WorldVertex>() as u64..),
                );
            } else {
                return;
            }
        } else if let Some(norm_o) = norm_o {
            if let Some(verts) = &*self.renderer.cache().normal_vertices {
                self.rpass.set_vertex_buffer(
                    1,
                    verts.slice(norm_o.id * mem::size_of::<NormalVertex>() as u64..),
                );
            } else {
                return;
            }
        }

        for range in ranges {
            self.rpass.draw_indexed(range, 0, 0..1);
        }

        if let PipelineDesc::Models {
            origin,
            data_offset,
        } = pipeline
        {
            let pipeline = self.renderer.rtlights_pipeline.as_ref().unwrap();
            self.rpass.set_pipeline(&pipeline.pipeline);
            self.rpass.set_bind_group(
                0,
                &pipeline.bind_group,
                &[u32::try_from(data_offset).unwrap()],
            );

            for range in ranges_for_lights.unwrap() {
                if range.len() > 0 {
                    for lights in self.world.lights(origin) {
                        if lights.len() > 0 {
                            self.rpass.draw_indexed(range.clone(), 0, lights.clone());
                        }
                    }
                }
            }

            self.cur_pipeline = None;
        }
    }
}

/// The view and projection matrices_buffer, used by the vertex shader.
/// Since we only split these out to make implementing the sky
/// shader simpler, the "projection matrix" actually includes
/// rotation - we just don't do translation.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct Matrices {
    /// Translation, in the Quake coordinate system
    translation: cgmath::Matrix4<f32>,
    /// The rest of the view/projection matrix
    projection: cgmath::Matrix4<f32>,
}

/// The uniforms used by the
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct FragmentUniforms {
    /// The amount to exponentiate the output colour by
    inv_gamma: f32,
    /// The amount to multiply the output colour by
    intensity: f32,
    /// To get the x coord of the current texture, do `texture.x + (animation frame % count) * texture.width`
    animation_frame: u32,
    /// So that animations know how much to offset by
    atlas_padding: u32,
    /// Level of ambient light, for model shading
    ambient_light: f32,
}

unsafe impl Pod for Matrices {}
unsafe impl Zeroable for Matrices {}

unsafe impl Pod for FragmentUniforms {}
unsafe impl Zeroable for FragmentUniforms {}

pub const MAX_LIGHTS: usize = 128;
const MINIMUM_ALIGNMENT: usize = 256;

impl Renderer {
    pub fn init(device: &wgpu::Device, out_size: (u32, u32), gamma: f32, intensity: f32) -> Self {
        assert_eq!(std::mem::size_of::<Light>(), 8 * 4);

        let cache = RenderCache::new(device);

        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sampler_diffuse"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 100.0,
            compare: wgpu::CompareFunction::Undefined,
        });

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sampler_diffuse_blurred"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            lod_min_clamp: 0.0,
            lod_max_clamp: 100.0,
            compare: wgpu::CompareFunction::Undefined,
        });

        let matrices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mat4_viewmatrix"),
            size: std::mem::size_of::<Matrices>() as u64,
            usage: wgpu::BufferUsage::UNIFORM | wgpu::BufferUsage::COPY_DST,
        });

        #[derive(Debug, Copy, Clone)]
        #[repr(C, align(256))]
        struct Count(u32);

        unsafe impl Pod for Count {}
        unsafe impl Zeroable for Count {}

        let mut counts = [Count(0u32); MAX_LIGHTS];

        for i in 0..MAX_LIGHTS {
            counts[i] = Count(i as u32);
        }

        let fragment_uniforms = FragmentUniforms {
            inv_gamma: gamma.recip(),
            intensity,
            animation_frame: 0,
            atlas_padding: 1,
            ambient_light: 0.0,
        };

        let fragment_uniforms_buffer = device.create_buffer_with_data(
            bytemuck::cast_slice(&[fragment_uniforms]),
            wgpu::BufferUsage::UNIFORM | wgpu::BufferUsage::COPY_DST,
        );

        let atlas_texture = [0, 0, 1, 1];
        let msaa_verts = device.create_buffer_with_data(
            bytemuck::cast_slice(&[
                TexturedVertex {
                    pos: [-1., -1., 0., 1.],
                    tex_coord: [0., 1.],
                    atlas_texture,
                },
                TexturedVertex {
                    pos: [1., -1., 0., 1.],
                    tex_coord: [1., 1.],
                    atlas_texture,
                },
                TexturedVertex {
                    pos: [1., 1., 0., 1.],
                    tex_coord: [1., 0.],
                    atlas_texture,
                },
                TexturedVertex {
                    pos: [1., 1., 0., 1.],
                    tex_coord: [1., 0.],
                    atlas_texture,
                },
                TexturedVertex {
                    pos: [-1., 1., 0., 1.],
                    tex_coord: [0., 0.],
                    atlas_texture,
                },
                TexturedVertex {
                    pos: [-1., -1., 0., 1.],
                    tex_coord: [0., 1.],
                    atlas_texture,
                },
            ]),
            wgpu::BufferUsage::VERTEX,
        );

        Self {
            out_size,
            matrices_buffer,
            fragment_uniforms,
            fragment_uniforms_dirty: false,
            fragment_uniforms_buffer,
            world_pipeline: None,
            skybox_pipeline: None,
            model_pipeline: None,
            rtlights_pipeline: None,
            post_pipeline: None,
            nearest_sampler,
            linear_sampler,
            cache,
            msaa_factor: NonZeroU8::new(1).unwrap(),
            msaa_buffer: None,
            msaa_verts,
            depth_buffer: None,
        }
    }

    pub fn set_size(&mut self, size: (u32, u32)) {
        if size != self.out_size {
            self.out_size = size;
            self.msaa_buffer = None;
            self.depth_buffer = None;
        }
    }

    pub fn set_msaa_factor(&mut self, factor: u8) {
        self.msaa_factor = NonZeroU8::new(factor).unwrap();
        if self
            .msaa_buffer
            .as_ref()
            .map(|buf| buf.factor)
            .unwrap_or(NonZeroU8::new(1).unwrap())
            != self.msaa_factor
        {
            self.msaa_buffer = None;
            self.depth_buffer = None;
        }
    }

    pub fn msaa_factor(&self) -> u8 {
        self.msaa_factor.get()
    }

    pub fn set_gamma(&mut self, gamma: f32) {
        self.fragment_uniforms.inv_gamma = gamma.recip();
        self.fragment_uniforms_dirty = true;
    }

    pub fn gamma(&self) -> f32 {
        self.fragment_uniforms.inv_gamma.recip()
    }

    pub fn set_intensity(&mut self, intensity: f32) {
        if self.fragment_uniforms.intensity != intensity {
            self.fragment_uniforms.intensity = intensity;
            self.fragment_uniforms_dirty = true;
        }
    }

    pub fn intensity(&self) -> f32 {
        self.fragment_uniforms.intensity
    }

    fn make_depth_tex(&self, device: &wgpu::Device) -> wgpu::TextureView {
        let depth_texture_desc = wgpu::TextureDescriptor {
            size: wgpu::Extent3d {
                width: self.out_size.0,
                height: self.out_size.1,
                depth: 1,
            },
            mip_level_count: 1,
            sample_count: 2 * self.msaa_factor() as u32,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsage::OUTPUT_ATTACHMENT,
            label: Some("tex_depth"),
        };

        let depth_texture = device.create_texture(&depth_texture_desc);

        depth_texture.create_default_view()
    }

    fn make_pipelines(&mut self, device: &wgpu::Device) {
        let diffuse_atlas_view = self.cache.diffuse.texture_view();
        let lightmap_atlas_view = self.cache.lightmap.texture_view();

        if self.world_pipeline.is_none() {
            self.world_pipeline = Some(pipelines::world::build(
                device,
                &diffuse_atlas_view,
                &lightmap_atlas_view,
                &self.nearest_sampler,
                &self.linear_sampler,
                &self.matrices_buffer,
                &self.fragment_uniforms_buffer,
                self.msaa_factor.get() as u32,
            ));
        }

        if self.skybox_pipeline.is_none() {
            self.skybox_pipeline = Some(pipelines::sky::build(
                device,
                &diffuse_atlas_view,
                &self.linear_sampler,
                &self.matrices_buffer,
                &self.fragment_uniforms_buffer,
                self.msaa_factor.get() as u32,
            ));
        }

        if self.model_pipeline.is_none() {
            self.model_pipeline = Some(pipelines::models::build(
                device,
                &diffuse_atlas_view,
                &self.linear_sampler,
                &self.matrices_buffer,
                &self.fragment_uniforms_buffer,
                self.cache.model_data.as_ref().unwrap(),
                self.msaa_factor.get() as u32,
            ));
        }

        if self.rtlights_pipeline.is_none() {
            self.rtlights_pipeline = Some(pipelines::rtlights::build(
                device,
                &self.matrices_buffer,
                &self.fragment_uniforms_buffer,
                self.cache.model_data.as_ref().unwrap(),
                self.msaa_factor.get() as u32,
            ));
        }
    }

    fn update_msaa_buffer(&mut self, device: &wgpu::Device) {
        if self.msaa_buffer.as_ref().map(|buf| buf.factor) != Some(self.msaa_factor) {
            let (width, height) = self.out_size;

            let buffer = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("msaa_diffuse_buffer"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth: 1,
                },
                mip_level_count: 1,
                sample_count: self.msaa_factor.get() as u32,
                dimension: wgpu::TextureDimension::D2,
                format: pipelines::postprocess::DIFFUSE_BUFFER_FORMAT,
                usage: wgpu::TextureUsage::SAMPLED | wgpu::TextureUsage::OUTPUT_ATTACHMENT,
            });

            let light_buffer = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("msaa_light_buffer"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth: 1,
                },
                mip_level_count: 1,
                sample_count: self.msaa_factor.get() as u32,
                dimension: wgpu::TextureDimension::D2,
                format: pipelines::postprocess::LIGHT_BUFFER_FORMAT,
                usage: wgpu::TextureUsage::SAMPLED | wgpu::TextureUsage::OUTPUT_ATTACHMENT,
            });

            self.msaa_buffer = Some(MsaaBuffer {
                factor: self.msaa_factor,
                diffuse_buffer: buffer.create_default_view(),
                light_buffer: light_buffer.create_default_view(),
            });

            self.skybox_pipeline = None;
            self.world_pipeline = None;
            self.model_pipeline = None;

            let MsaaBuffer {
                diffuse_buffer,
                light_buffer,
                ..
            } = self.msaa_buffer.as_ref().unwrap();
            self.post_pipeline = Some(pipelines::postprocess::build(
                device,
                diffuse_buffer,
                light_buffer,
                &self.linear_sampler,
                &self.fragment_uniforms_buffer,
            ));
        }
    }

    pub fn cache_mut(&mut self) -> &mut RenderCache {
        &mut self.cache
    }

    pub fn cache(&self) -> &RenderCache {
        &self.cache
    }

    pub fn advance_frame(&mut self) {
        self.fragment_uniforms.animation_frame += 1;
        self.fragment_uniforms_dirty = true;
    }

    pub fn transfer_data<I>(&mut self, queue: &wgpu::Queue, items: I)
    where
        I: IntoIterator,
        I::Item: TransferData,
    {
        if let Some(model_data) = self.cache.model_data.as_ref() {
            for i in items {
                if let Some((offset, data)) = i.transfer_data() {
                    // TODO: We can batch this
                    queue.write_buffer(&model_data, offset, bytemuck::bytes_of(&data));
                }
            }
        }
    }

    pub fn render<'a, F>(
        &mut self,
        device: &wgpu::Device,
        camera: &Camera,
        screen_tex: &wgpu::TextureView,
        queue: &wgpu::Queue,
        render: F,
    ) -> impl Iterator<Item = wgpu::CommandBuffer>
    where
        F: FnOnce(RenderContext<'_>),
    {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render"),
        });
        self.cache.update(device, &mut encoder);

        self.update_msaa_buffer(device);
        self.make_pipelines(device);

        let indices = if let Some(i) = self.cache.indices.as_ref() {
            i
        } else {
            return None.into_iter();
        };

        let translation = camera.translation();
        let projection = camera.projection();
        queue.write_buffer(
            &self.matrices_buffer,
            0,
            bytemuck::cast_slice(&[Matrices {
                translation,
                projection,
            }]),
        );

        if self.fragment_uniforms_dirty {
            self.fragment_uniforms_dirty = false;
            queue.write_buffer(
                &self.fragment_uniforms_buffer,
                0,
                bytemuck::cast_slice(&[self.fragment_uniforms]),
            );
        }

        let depth = if let Some(depth) = &self.depth_buffer {
            depth
        } else {
            self.depth_buffer = Some(self.make_depth_tex(device));
            self.depth_buffer.as_ref().unwrap()
        };

        {
            let MsaaBuffer {
                diffuse_buffer,
                light_buffer,
                ..
            } = self.msaa_buffer.as_ref().unwrap();

            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[
                    wgpu::RenderPassColorAttachmentDescriptor {
                        attachment: diffuse_buffer,
                        resolve_target: None,
                        load_op: wgpu::LoadOp::Load,
                        store_op: wgpu::StoreOp::Store,
                        clear_color: wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                    },
                    wgpu::RenderPassColorAttachmentDescriptor {
                        attachment: light_buffer,
                        resolve_target: None,
                        load_op: wgpu::LoadOp::Clear,
                        store_op: wgpu::StoreOp::Store,
                        clear_color: wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        },
                    },
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachmentDescriptor {
                    attachment: &depth,
                    depth_load_op: wgpu::LoadOp::Clear,
                    depth_store_op: wgpu::StoreOp::Store,
                    stencil_load_op: wgpu::LoadOp::Clear,
                    stencil_store_op: wgpu::StoreOp::Store,
                    clear_depth: 1.0,
                    clear_stencil: 0,
                }),
            });

            rpass.set_index_buffer(indices.slice(..));
            rpass.set_vertex_buffer(2, self.cache().lights.as_ref().unwrap().slice(..));

            render(RenderContext {
                renderer: self,
                camera,
                rpass,
                world: (),
                cur_pipeline: None,
            });
        }

        {
            let mut post_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[wgpu::RenderPassColorAttachmentDescriptor {
                    attachment: screen_tex,
                    resolve_target: None,
                    load_op: wgpu::LoadOp::Load,
                    store_op: wgpu::StoreOp::Store,
                    clear_color: wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    },
                }],
                depth_stencil_attachment: None,
            });

            post_pass.set_pipeline(&self.post_pipeline.as_ref().unwrap().pipeline);
            post_pass.set_bind_group(0, &self.post_pipeline.as_ref().unwrap().bind_group, &[]);
            post_pass.set_vertex_buffer(0, self.msaa_verts.slice(..));
            post_pass.draw(0..6, 0..1);
        }

        Some(encoder.finish()).into_iter()
    }
}
