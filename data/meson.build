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

# What app stores show (GNOME Software, Flathub), translated like the rest.
metainfo = i18n.merge_file(
  input: application_id + '.metainfo.xml.in',
  output: application_id + '.metainfo.xml',
  po_dir: meson.project_source_root() / 'po',
  install: true,
  install_dir: get_option('datadir') / 'metainfo',
)

appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test(
    'validate-metainfo',
    appstreamcli,
    args: ['validate', '--no-net', '--explain', metainfo],
  )
endif

# The application icon in every size, and its symbolic variant.
subdir('icons')

# Symbolic icons the app uses inside (the Radio button), next to the app icon.
install_data(
  'icons/vumi-radio-symbolic.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)
