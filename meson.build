project(
  'muse-player',
  version: '0.1.0',
  meson_version: '>= 1.0',
)

gnome = import('gnome')

application_id = 'io.github.tomaszbojanowski.MusePlayer'

subdir('data')

# Meson drives cargo; the binary is copied into the build dir and installed.
cargo = find_program('cargo')
cargo_target_dir = meson.project_build_root() / 'target'

custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  command: [
    cargo, 'build', '--release',
    '--manifest-path', meson.project_source_root() / 'Cargo.toml',
    '--target-dir', cargo_target_dir,
    '-p', 'app',
    '&&',
    'cp', cargo_target_dir / 'release' / meson.project_name(), '@OUTPUT@',
  ],
)

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
