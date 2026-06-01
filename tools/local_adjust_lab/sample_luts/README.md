# local_adjust_lab sample LUTs

These `.cube` files are small self-made 3D LUT samples for testing the
local-adjustment 3D LUT effect.

- `identity.cube`: no visible change, useful for loader tests.
- `warm_sunset.cube`: warm orange highlights and reduced blue.
- `cool_moonlight.cube`: cool blue shadows and highlights.
- `soft_film.cube`: lifted blacks and slightly faded highlights.
- `vivid_pop.cube`: saturated primary-color pop look.

They are intentionally tiny (`LUT_3D_SIZE 2`) so they are easy to inspect and
safe to keep in the repository. They are authored for this project and can be
redistributed with mIV.
