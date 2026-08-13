export async function formatBalanceScript(source: string): Promise<string> {
  const [{ default: prettier }, babel, estree] = await Promise.all([
    import("prettier/standalone"),
    import("prettier/plugins/babel"),
    import("prettier/plugins/estree"),
  ]);
  return prettier.format(source, {
    parser: "babel",
    plugins: [babel, estree],
    printWidth: 88,
    semi: true,
    singleQuote: false,
    tabWidth: 2,
  });
}
