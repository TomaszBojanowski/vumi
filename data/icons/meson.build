# Install the VU application icons. application_id comes from the project root.
# Flatpak exports app icons up to 512x512, so there is no larger size.
foreach icon_size : ['16', '24', '32', '48', '64', '96', '128', '192', '256', '512']
  icon_size_dir = icon_size + 'x' + icon_size
  install_data(
    'hicolor' / icon_size_dir / 'apps' / (application_id + '.png'),
    install_dir: get_option('datadir') / 'icons' / 'hicolor' / icon_size_dir / 'apps',
  )
endforeach

install_data(
  'hicolor' / 'symbolic' / 'apps' / (application_id + '-symbolic.svg'),
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)
