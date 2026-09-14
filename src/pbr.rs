//! Optional extended PBR inputs, shared by the asset loader and editor preview.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alpha_constant_roundtrips_and_updates_material() {
        let p = BmatPbr { alpha_factor: Some(0.25), ..Default::default() };
        let encoded = ron::ser::to_string(&p).unwrap();
        let reopened: BmatPbr = ron::de::from_str(&encoded).unwrap();
        assert_eq!(reopened, p);
        let mut material = StandardMaterial::default();
        reopened.apply(&mut material, |_| panic!("No textures expected")).unwrap();
        assert_eq!(material.base_color.alpha(), 0.25);
        for v in [-0.1, 1.1, f32::NAN] {
            assert!(BmatPbr { alpha_factor: Some(v), ..Default::default() }.validate().is_err());
        }
    }
    #[test]
    fn unset_values_preserve_bevy_defaults() {
        let mut m = StandardMaterial::default();
        let expected = StandardMaterial::default();
        BmatPbr::default()
            .apply(&mut m, |_| panic!("None must not load textures"))
            .unwrap();
        assert_eq!(m.clearcoat, expected.clearcoat);
        assert_eq!(
            m.clearcoat_perceptual_roughness,
            expected.clearcoat_perceptual_roughness
        );
        assert_eq!(m.diffuse_transmission, expected.diffuse_transmission);
        assert_eq!(m.specular_transmission, expected.specular_transmission);
        assert_eq!(m.thickness, expected.thickness);
        assert_eq!(m.ior, expected.ior);
        assert_eq!(m.attenuation_distance, expected.attenuation_distance);
        assert_eq!(m.anisotropy_strength, expected.anisotropy_strength);
        assert_eq!(m.anisotropy_rotation, expected.anisotropy_rotation);
        assert_eq!(ron::ser::to_string(&BmatPbr::default()).unwrap(), "()");
    }
    #[test]
    fn factors_and_linear_textures_are_applied() {
        let p = BmatPbr {
            clearcoat: Some(0.6),
            specular_transmission: Some(0.8),
            ior: Some(1.33),
            thickness: Some(2.),
            anisotropy_strength: Some(0.7),
            anisotropy_rotation: Some(0.25),
            clearcoat_texture: Some("coat.ktx2".into()),
            attenuation_color: Some([0.1, 0.2, 0.3]),
            ..Default::default()
        };
        let mut m = StandardMaterial::default();
        let mut loaded = Vec::new();
        p.apply(&mut m, |path| {
            loaded.push(path.to_string());
            Ok(Handle::default())
        })
        .unwrap();
        assert_eq!(loaded, vec!["coat.ktx2"]);
        assert_eq!(m.clearcoat, 0.6);
        assert_eq!(m.specular_transmission, 0.8);
        assert_eq!(m.ior, 1.33);
        assert_eq!(m.thickness, 2.);
        assert_eq!(m.anisotropy_strength, 0.7);
        assert_eq!(m.anisotropy_rotation, 0.25);
        assert_eq!(m.attenuation_color, Color::linear_rgb(0.1, 0.2, 0.3));
    }
    #[test]
    fn invalid_factors_are_rejected() {
        for p in [
            BmatPbr {
                ior: Some(0.),
                ..Default::default()
            },
            BmatPbr {
                clearcoat: Some(f32::NAN),
                ..Default::default()
            },
            BmatPbr {
                anisotropy_strength: Some(1.1),
                ..Default::default()
            },
            BmatPbr {
                attenuation_distance: Some(0.),
                ..Default::default()
            },
        ] {
            assert!(p.validate().is_err());
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BmatPbr {
    /// Multiplies the base-color texture alpha; alpha mode controls interpretation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha_factor: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clearcoat: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clearcoat_perceptual_roughness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diffuse_transmission: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specular_transmission: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thickness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ior: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attenuation_distance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anisotropy_strength: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anisotropy_rotation: Option<f32>,
    /// Linear RGB transmittance at the attenuation distance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attenuation_color: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clearcoat_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clearcoat_roughness_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clearcoat_normal_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diffuse_transmission_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specular_transmission_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thickness_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anisotropy_texture: Option<String>,
}
impl BmatPbr {
    pub fn validate(&self) -> Result<(), String> {
        if self.alpha_factor.is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v)) {
            return Err("Alpha must be finite and between 0 and 1".into());
        }
        if self
            .clearcoat
            .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err("Invalid clearcoat".into());
        }
        if self
            .clearcoat_perceptual_roughness
            .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err("Invalid clearcoat_perceptual_roughness".into());
        }
        if self
            .diffuse_transmission
            .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err("Invalid diffuse_transmission".into());
        }
        if self
            .specular_transmission
            .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err("Invalid specular_transmission".into());
        }
        if self
            .thickness
            .is_some_and(|v| !v.is_finite() || !(0.0..=f32::MAX).contains(&v))
        {
            return Err("Invalid thickness".into());
        }
        if self
            .ior
            .is_some_and(|v| !v.is_finite() || !(1.0..=f32::MAX).contains(&v))
        {
            return Err("Invalid ior".into());
        }
        if self
            .attenuation_distance
            .is_some_and(|v| !v.is_finite() || !(f32::MIN_POSITIVE..=f32::MAX).contains(&v))
        {
            return Err("Invalid attenuation_distance".into());
        }
        if self
            .anisotropy_strength
            .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err("Invalid anisotropy_strength".into());
        }
        if self
            .anisotropy_rotation
            .is_some_and(|v| !v.is_finite() || !(-f32::MAX..=f32::MAX).contains(&v))
        {
            return Err("Invalid anisotropy_rotation".into());
        }
        if self
            .attenuation_color
            .is_some_and(|c| c.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)))
        {
            return Err("Attenuation color must be finite linear RGB in 0–1".into());
        }
        Ok(())
    }

    /// None leaves StandardMaterial's existing default untouched.
    pub fn apply(
        &self,
        material: &mut StandardMaterial,
        mut image: impl FnMut(&str) -> Result<Handle<Image>, String>,
    ) -> Result<(), String> {
        self.validate()?;
        if let Some(alpha) = self.alpha_factor { material.base_color = material.base_color.with_alpha(alpha); }
        if let Some(v) = self.clearcoat {
            material.clearcoat = v;
        }
        if let Some(v) = self.clearcoat_perceptual_roughness {
            material.clearcoat_perceptual_roughness = v;
        }
        if let Some(v) = self.diffuse_transmission {
            material.diffuse_transmission = v;
        }
        if let Some(v) = self.specular_transmission {
            material.specular_transmission = v;
        }
        if let Some(v) = self.thickness {
            material.thickness = v;
        }
        if let Some(v) = self.ior {
            material.ior = v;
        }
        if let Some(v) = self.attenuation_distance {
            material.attenuation_distance = v;
        }
        if let Some(v) = self.anisotropy_strength {
            material.anisotropy_strength = v;
        }
        if let Some(v) = self.anisotropy_rotation {
            material.anisotropy_rotation = v;
        }
        if let Some([r, g, b]) = self.attenuation_color {
            material.attenuation_color = Color::linear_rgb(r, g, b);
        }
        if let Some(path) = &self.clearcoat_texture {
            material.clearcoat_texture = Some(image(path)?);
        }
        if let Some(path) = &self.clearcoat_roughness_texture {
            material.clearcoat_roughness_texture = Some(image(path)?);
        }
        if let Some(path) = &self.clearcoat_normal_texture {
            material.clearcoat_normal_texture = Some(image(path)?);
        }
        if let Some(path) = &self.diffuse_transmission_texture {
            material.diffuse_transmission_texture = Some(image(path)?);
        }
        if let Some(path) = &self.specular_transmission_texture {
            material.specular_transmission_texture = Some(image(path)?);
        }
        if let Some(path) = &self.thickness_texture {
            material.thickness_texture = Some(image(path)?);
        }
        if let Some(path) = &self.anisotropy_texture {
            material.anisotropy_texture = Some(image(path)?);
        }
        Ok(())
    }

    pub fn textures_mut(&mut self) -> [(&'static str, &mut Option<String>); 7] {
        [
            ("clearcoat_texture", &mut self.clearcoat_texture),
            (
                "clearcoat_roughness_texture",
                &mut self.clearcoat_roughness_texture,
            ),
            (
                "clearcoat_normal_texture",
                &mut self.clearcoat_normal_texture,
            ),
            (
                "diffuse_transmission_texture",
                &mut self.diffuse_transmission_texture,
            ),
            (
                "specular_transmission_texture",
                &mut self.specular_transmission_texture,
            ),
            ("thickness_texture", &mut self.thickness_texture),
            ("anisotropy_texture", &mut self.anisotropy_texture),
        ]
    }
}
