install_data(
  application_id + '.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

i18n.merge_file(
  input: application_id + '.desktop.in',
  output: application_id + '.desktop',
  type: 'desktop',
  po_dir: meson.project_source_root() / 'po',
  install: true,
  install_dir: get_option('datadir') / 'applications',
)

install_data(
  'icons' / (application_id + '.svg'),
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
)

# Symbolic icons the app uses inside (the Radio button), next to the app icon.
install_data(
  'icons/muse-player-radio-symbolic.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)
