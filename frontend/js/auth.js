function base64url(bytes) {
  let binary = "";
  bytes.forEach((byte) => {
    binary += String.fromCharCode(byte);
  });
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

export async function beginLogin(role) {
  if (window.ST_CONFIG.mock) return;
  const verifierBytes = new Uint8Array(32);
  crypto.getRandomValues(verifierBytes);
  const verifier = base64url(verifierBytes);
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  const challenge = base64url(new Uint8Array(digest));
  sessionStorage.setItem("st-pkce", verifier);
  sessionStorage.setItem("st-role", role);
  const redirect = new URL("auth/callback/", window.location.origin).href;
  const url = new URL(`https://${window.ST_CONFIG.cognitoDomain}/oauth2/authorize`);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("client_id", window.ST_CONFIG.clientId);
  url.searchParams.set("redirect_uri", redirect);
  url.searchParams.set("scope", "openid email");
  url.searchParams.set("code_challenge", challenge);
  url.searchParams.set("code_challenge_method", "S256");
  window.location.assign(url.href);
}

export async function signUp(role, email, password) {
  if (window.ST_CONFIG.mock) return { mock: true };
  const response = await fetch(`https://cognito-idp.${window.ST_CONFIG.region}.amazonaws.com/`, {
    method: "POST",
    headers: {
      "content-type": "application/x-amz-json-1.1",
      "x-amz-target": "AWSCognitoIdentityProviderService.SignUp",
    },
    body: JSON.stringify({
      ClientId: window.ST_CONFIG.clientId,
      Username: email,
      Password: password,
      UserAttributes: [
        { Name: "email", Value: email },
        { Name: "custom:role", Value: role },
      ],
    }),
  });
  if (!response.ok) throw new Error("signup_failed");
  sessionStorage.setItem("st-pending-user", email);
  return response.json();
}

export async function confirmSignUp(code) {
  const username = sessionStorage.getItem("st-pending-user");
  const response = await fetch(`https://cognito-idp.${window.ST_CONFIG.region}.amazonaws.com/`, {
    method: "POST",
    headers: {
      "content-type": "application/x-amz-json-1.1",
      "x-amz-target": "AWSCognitoIdentityProviderService.ConfirmSignUp",
    },
    body: JSON.stringify({
      ClientId: window.ST_CONFIG.clientId,
      Username: username,
      ConfirmationCode: code,
    }),
  });
  if (!response.ok) throw new Error("confirm_failed");
  return response.json();
}

export async function finishLogin(code) {
  const verifier = sessionStorage.getItem("st-pkce");
  const redirect = new URL("auth/callback/", window.location.origin).href;
  const body = new URLSearchParams({
    grant_type: "authorization_code",
    client_id: window.ST_CONFIG.clientId,
    code,
    redirect_uri: redirect,
    code_verifier: verifier || "",
  });
  const response = await fetch(`https://${window.ST_CONFIG.cognitoDomain}/oauth2/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body,
  });
  if (!response.ok) throw new Error("token_failed");
  const tokens = await response.json();
  sessionStorage.setItem("st-access-token", tokens.access_token);
  sessionStorage.setItem("st-refresh-token", tokens.refresh_token);
  sessionStorage.removeItem("st-pkce");
}

export async function logout() {
  const refresh = sessionStorage.getItem("st-refresh-token");
  if (!window.ST_CONFIG.mock && refresh && window.ST_CONFIG.cognitoDomain) {
    await fetch(`https://${window.ST_CONFIG.cognitoDomain}/oauth2/revoke`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ token: refresh, client_id: window.ST_CONFIG.clientId }),
    }).catch(() => {});
  }
  sessionStorage.removeItem("st-access-token");
  sessionStorage.removeItem("st-refresh-token");
  if (!window.ST_CONFIG.mock && window.ST_CONFIG.cognitoDomain) {
    const url = new URL(`https://${window.ST_CONFIG.cognitoDomain}/logout`);
    url.searchParams.set("client_id", window.ST_CONFIG.clientId);
    url.searchParams.set("logout_uri", window.location.origin + "/");
    window.location.assign(url.href);
  }
}
