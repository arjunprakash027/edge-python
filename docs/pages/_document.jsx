import { Html, Head, Main, NextScript } from 'next/document'

// Only here to set <html lang>, which can't be set from next/head and which
// `output: 'export'` won't get from next.config i18n.
export default function Document() {
  return (
    <Html lang="en">
      <Head />
      <body>
        <Main />
        <NextScript />
      </body>
    </Html>
  )
}
