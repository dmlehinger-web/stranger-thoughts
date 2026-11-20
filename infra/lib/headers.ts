export interface CspHosts {
  cognitoDomain: string;
  region: string;
}

/**
 * Production CSP. The browser also enforces a slightly wider meta tag on the
 * static files (so a generated API hostname still fits). CloudFront sends this
 * exact policy, and both policies have to allow a request.
 */
export function contentSecurityPolicy(hosts: CspHosts): string {
  const cognitoHost = `https://${hosts.cognitoDomain}.auth.${hosts.region}.amazoncognito.com`;
  const idpHost = `https://cognito-idp.${hosts.region}.amazonaws.com`;
  return [
    "default-src 'self'",
    "script-src 'self'",
    "style-src 'self'",
    "font-src 'self'",
    "img-src 'self'",
    `connect-src 'self' ${cognitoHost} ${idpHost}`,
    "object-src 'none'",
    "base-uri 'self'",
    "frame-ancestors 'none'",
    "form-action 'self' https://checkout.stripe.com https://connect.stripe.com",
  ].join("; ");
}

export const PERMISSIONS_POLICY = "camera=(), microphone=(), geolocation=(), payment=(), usb=()";

export const SECURITY_HEADER_NAMES = [
  "Strict-Transport-Security",
  "Content-Security-Policy",
  "X-Content-Type-Options",
  "Referrer-Policy",
  "X-Frame-Options",
  "Permissions-Policy",
] as const;
