/** @type {import('next').NextConfig} */
module.exports = {
  outputFileTracingRoot: __dirname,
  distDir: process.env.NODE_ENV === "development" ? ".next-dev" : ".next",
  output: "standalone",
  poweredByHeader: false,
  experimental: { cpus: 2 },
};
