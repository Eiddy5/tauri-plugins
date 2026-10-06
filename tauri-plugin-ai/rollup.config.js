import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { cwd } from 'node:process'
import typescript from '@rollup/plugin-typescript'

const pkg = JSON.parse(readFileSync(join(cwd(), 'package.json'), 'utf8'))

export default {
  input: {
    index: 'guest-js/index.ts',
    tauri: 'guest-js/tauri.ts',
    angular: 'guest-js/angular.ts',
  },
  treeshake: { moduleSideEffects: false },
  output: [
    {
      dir: 'dist-js',
      entryFileNames: '[name].js',
      chunkFileNames: 'shared/[name].js',
      hoistTransitiveImports: false,
      format: 'esm',
    },
    {
      dir: 'dist-js',
      entryFileNames: '[name].cjs',
      chunkFileNames: 'shared/[name].cjs',
      hoistTransitiveImports: false,
      format: 'cjs',
    },
  ],
  plugins: [
    typescript({
      declaration: true,
      declarationDir: 'dist-js',
    }),
  ],
  external: [
    /^@tauri-apps\/api/,
    /^ajv\//,
    ...Object.keys(pkg.dependencies || {}),
    ...Object.keys(pkg.peerDependencies || {}),
  ],
}
