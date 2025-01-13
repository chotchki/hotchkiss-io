htmx.defineExtension('webauthn-autofill', {
    init: function (api) {
        console.log("Fired Webauthn Autofill check");
        (async () => {
            if (
                typeof window.PublicKeyCredential !== 'undefined'
                && typeof window.PublicKeyCredential.isConditionalMediationAvailable === 'function'
            ) {
                const available = await PublicKeyCredential.isConditionalMediationAvailable();

                if (available) {
                    try {
                        // Retrieve authentication options for `navigator.credentials.get()`
                        // from your server.
                        const response = await fetch("/login/getAuthOptions");
                        if (!response.ok) {
                            throw new Error('Response from auth options: ${response.status}');
                        }

                        let authOptions = await response.json();
                        authOptions["mediation"] = "conditional";
                        if (typeof window.PublicKeyCredential.parseCreationOptionsFromJSON === 'function') {
                            authOptions = PublicKeyCredential.parseCreationOptionsFromJSON(authOptions);
                        } else {
                            //Due to a Safari bug, hand create our authOptions object
                            //note we're using a fromBase64 function is not availible in Chrome yet so fingers crossed
                            authOptions["publicKey"]["challenge"] = Uint8Array.fromBase64(authOptions["publicKey"]["challenge"], { alphabet: 'base64url' });
                        }

                        const webAuthnResponse = await navigator.credentials.get(authOptions);

                        // Send the response to your server for verification and
                        // authenticate the user if the response is valid.
                        await verifyAutoFillResponse(webAuthnResponse);
                    } catch (err) {
                        console.error('Error with conditional UI:', err);
                    }
                }
            }
        })();
    }
});

htmx.defineExtension('webauthn-register', {
    onEvent: function (name, evt) {
        if (name !== "htmx:beforeRequest") {
            return;
        }
        console.log("Fired Webauthn Register");

    }
});