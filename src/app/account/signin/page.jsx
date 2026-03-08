"use client";

import { useState } from "react";
import useAuth from "@/utils/useAuth";

export default function SignInPage() {
  const [error, setError] = useState(null);
  const [loading, setLoading] = useState(false);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");

  const { signInWithCredentials, signInWithGoogle } = useAuth();

  const onSubmit = async (e) => {
    e.preventDefault();
    setLoading(true);
    setError(null);

    if (!email || !password) {
      setError("Please fill in all fields");
      setLoading(false);
      return;
    }

    try {
      await signInWithCredentials({
        email,
        password,
        callbackUrl: "/",
        redirect: true,
      });
    } catch (err) {
      const errorMessages = {
        OAuthSignin: "Couldn't start sign-in. Please try again or use a different method.",
        OAuthCallback: "Sign-in failed after redirecting. Please try again.",
        OAuthCreateAccount: "Couldn't create an account with this sign-in method. Try another option.",
        EmailCreateAccount: "This email can't be used to create an account. It may already exist.",
        Callback: "Something went wrong during sign-in. Please try again.",
        OAuthAccountNotLinked: "This account is linked to a different sign-in method. Try using that instead.",
        CredentialsSignin: "Incorrect email or password. Try again or reset your password.",
        AccessDenied: "You don't have permission to sign in.",
        Configuration: "Sign-in isn't working right now. Please try again later.",
        Verification: "Your sign-in link has expired. Request a new one.",
      };

      setError(errorMessages[err.message] || "Something went wrong. Please try again.");
      setLoading(false);
    }
  };

  const handleGoogleSignIn = async () => {
    setLoading(true);
    setError(null);
    try {
      await signInWithGoogle({
        callbackUrl: "/",
        redirect: true,
      });
    } catch (err) {
      setError("Google sign-in failed. Please try again.");
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen bg-white dark:bg-[#121212] flex items-center justify-center p-4">
      <div className="w-full max-w-md">
        {/* Logo */}
        <div className="text-center mb-8">
          <div className="flex items-center justify-center space-x-2 mb-4">
            <img
              src="https://ucarecdn.com/79c8b717-b6bd-44b8-bb53-45639ed9697f/-/format/auto/"
              alt="BoopMark Logo"
              className="w-10 h-10"
            />
            <span className="font-bricolage text-2xl font-bold text-[#0E1B36] dark:text-white">
              BoopMark
            </span>
          </div>
          <h1 className="font-bricolage text-xl font-bold text-gray-900 dark:text-white mb-2">
            Welcome Back
          </h1>
          <p className="font-inter text-gray-600 dark:text-gray-400">
            Sign in to your bookmark collection
          </p>
        </div>

        <div className="bg-gray-50 dark:bg-[#1E1E1E] rounded-xl p-6 border border-gray-200 dark:border-gray-700">
          {/* Google Sign In Button */}
          <button
            onClick={handleGoogleSignIn}
            disabled={loading}
            className="w-full mb-4 bg-white dark:bg-[#262626] border border-gray-300 dark:border-gray-600 text-gray-700 dark:text-gray-300 py-3 px-4 rounded-lg font-inter font-medium hover:bg-gray-50 dark:hover:bg-[#2A2A2A] disabled:opacity-50 flex items-center justify-center gap-3"
          >
            <svg width="18" height="18" viewBox="0 0 18 18">
              <path fill="#4285F4" d="M16.51 8H8.98v3h4.3c-.18 1-.74 1.48-1.6 2.04v1.7h2.6c1.53-1.4 2.41-3.5 2.41-6z"/>
              <path fill="#34A853" d="M8.98 17c2.16 0 3.97-.72 5.3-1.94l-2.6-1.7c-.72.49-1.63.78-2.7.78-2.08 0-3.84-1.4-4.48-3.29H1.96v1.75C3.28 15.29 5.92 17 8.98 17z"/>
              <path fill="#FBBC05" d="M4.5 10.85c-.16-.49-.25-1.02-.25-1.55s.09-1.06.25-1.55V5.99H1.96C1.35 7.2 1 8.54 1 9.9s.35 2.7.96 3.91l2.54-1.96z"/>
              <path fill="#EA4335" d="M8.98 3.86c1.17 0 2.23.4 3.06 1.2l2.3-2.3C12.94.99 11.13 0 8.98 0 5.92 0 3.28 1.71 1.96 4.29L4.5 6.05C5.14 4.16 6.9 2.76 8.98 2.76z"/>
            </svg>
            Continue with Google
          </button>

          <div className="relative mb-4">
            <div className="absolute inset-0 flex items-center">
              <div className="w-full border-t border-gray-300 dark:border-gray-600"></div>
            </div>
            <div className="relative flex justify-center text-sm">
              <span className="px-2 bg-gray-50 dark:bg-[#1E1E1E] text-gray-500 dark:text-gray-400 font-inter">
                or continue with email
              </span>
            </div>
          </div>

          <form onSubmit={onSubmit} className="space-y-4">
            <div>
              <label className="block text-sm font-inter font-medium text-gray-700 dark:text-gray-300 mb-1">
                Email
              </label>
              <input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="your@email.com"
                required
                className="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#262626] text-gray-900 dark:text-white rounded-lg focus:outline-none focus:ring-2 focus:ring-[#7D65FF] dark:focus:ring-[#8B7AFF] font-inter text-sm"
              />
            </div>

            <div>
              <label className="block text-sm font-inter font-medium text-gray-700 dark:text-gray-300 mb-1">
                Password
              </label>
              <input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="Enter your password"
                required
                className="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#262626] text-gray-900 dark:text-white rounded-lg focus:outline-none focus:ring-2 focus:ring-[#7D65FF] dark:focus:ring-[#8B7AFF] font-inter text-sm"
              />
            </div>

            {error && (
              <div className="bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded-lg p-3">
                <p className="text-red-700 dark:text-red-400 text-sm font-inter">{error}</p>
              </div>
            )}

            <button
              type="submit"
              disabled={loading}
              className="w-full bg-[#7D65FF] dark:bg-[#8B7AFF] hover:bg-[#6B56E8] dark:hover:bg-[#7A6AEF] active:bg-[#5A4BDB] dark:active:bg-[#695CE5] text-white font-inter font-semibold py-2 px-4 rounded-lg disabled:opacity-50 transition-colors"
            >
              {loading ? "Signing in..." : "Sign In"}
            </button>
          </form>

          <p className="text-center text-sm text-gray-600 dark:text-gray-400 font-inter mt-4">
            Don't have an account?{" "}
            <a
              href={`/account/signup${typeof window !== "undefined" ? window.location.search : ""}`}
              className="text-[#7D65FF] dark:text-[#8B7AFF] hover:underline"
            >
              Sign up
            </a>
          </p>
        </div>
      </div>

      {/* Google Fonts */}
      <style jsx global>{`
        @import url('https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,800&family=Inter:wght@400;500;600;700&display=swap');
        
        .font-bricolage {
          font-family: 'Bricolage Grotesque', sans-serif;
        }
        
        .font-inter {
          font-family: 'Inter', sans-serif;
        }
      `}</style>
    </div>
  );
}