project(
  'vumi',
  version: '0.1.0',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 1.0',
)

gnome = import('gnome')
i18n = import('i18n')

application_id = 'io.github.tomaszbojanowski.Vumi'
localedir = get_option('prefix') / get_option('localedir')

subdir('data')
subdir('po')

# Meson drives cargo; the binary is copied into the build dir and installed.
# LOCALEDIR is read by the Rust code at compile time (option_env!).
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
  # Cargo build and copy in one script: Meson runs commands without a shell.
  command: [
    find_program('build-aux/cargo-build.sh'),
    cargo,
    meson.project_source_root() / 'Cargo.toml',
    cargo_target_dir,
    localedir,
    meson.project_name(),
    '@OUTPUT@',
  ],
)

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
