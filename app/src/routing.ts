export type AppRouteName = 'main' | 'overlay'

export function routeForPath(pathname: string): AppRouteName {
  if (pathname === '/overlay') return 'overlay'
  return 'main'
}
