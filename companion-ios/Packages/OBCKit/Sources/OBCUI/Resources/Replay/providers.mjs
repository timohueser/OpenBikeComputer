// Owner development only. Public distribution must supply licensed data access.
export async function createProviders(C) {
  const [terrain, imagery] = await Promise.all([
    C.ArcGISTiledElevationTerrainProvider.fromUrl('https://elevation3d.arcgis.com/arcgis/rest/services/WorldElevation3D/Terrain3D/ImageServer'),
    C.ArcGisMapServerImageryProvider.fromUrl('https://services.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer', { enablePickFeatures: false }),
  ]);
  return { terrain, imagery };
}
