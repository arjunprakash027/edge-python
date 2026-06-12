/** @type {import('tailwindcss').Config} */
export default {
  // Only the playground markup uses Tailwind; nextra-theme-docs owns everything else.
  content: ['./components/**/*.{js,jsx}', './mdx-components.jsx'],
  // Match Nextra/next-themes (attribute: "class" -> <html class="dark">) so our `dark:` utilities follow the docs' theme toggle, not the OS preference. Without this the header never switches.
  darkMode: 'class',
  // No preflight: Tailwind's reset would clash with the Nextra theme. Utilities only.
  corePlugins: { preflight: false },
  theme: { extend: {} },
  plugins: [],
}
