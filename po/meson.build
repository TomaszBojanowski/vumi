i18n.gettext(
  meson.project_name(),
  args: [
    '--from-code=UTF-8',
    '--add-comments=Translators',
    '--keyword=gettext',
    '--keyword=_',
    '--keyword=ngettext:1,2',
    # With a context, for a text that reads differently in two places:
    # pgettext("context", "text") in Rust, C_("context", "text") in Blueprint.
    '--keyword=pgettext:1c,2',
    '--keyword=C_:1c,2',
  ],
)
