# Code signing

Releases are published unsigned unless the repository has signing secrets; the release workflow then signs the .exe
with Azure Trusted Signing before computing the checksums.

1. In the Azure portal, create a Trusted Signing account and a certificate profile (Public Trust), and complete the
   identity validation.
2. Create an app registration (Microsoft Entra ID) with a client secret, and give it the "Trusted Signing Certificate
   Profile Signer" role on the account.
3. In this repository, Settings > Secrets and variables > Actions, add: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`,
   `AZURE_CLIENT_SECRET`, `SIGNING_ENDPOINT` (e.g. `https://weu.codesigning.azure.net/`), `SIGNING_ACCOUNT` (the
   account name) and `SIGNING_PROFILE` (the certificate profile name).

The next release is then signed. Alternative for open-source projects: SignPath.io's free plan (another workflow
step).
