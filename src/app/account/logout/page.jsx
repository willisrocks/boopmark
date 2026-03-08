"use client";

import { useState, useEffect } from "react";
import useAuth from "@/utils/useAuth";

export default function LogoutPage() {
  const [loading, setLoading] = useState(false);
  const { signOut } = useAuth();

  const handleSignOut = async () => {
    setLoading(true);
    try {
      await signOut({
        callbackUrl: "/",
        redirect: true,
      });
    } catch (error) {
      console.error("Sign out error:", error);
      setLoading(false);
    }
  };

  useEffect(() => {
    // Auto sign out when the page loads
    handleSignOut();
  }, []);

  return (
    <div className="min-h-screen bg-white dark:bg-[#121212] flex items-center justify-center p-4">
      <div className="text-center max-w-md">
        <div className="flex items-center justify-center space-x-2 mb-6">
          <img
            src="https://ucarecdn.com/79c8b717-b6bd-44b8-bb53-45639ed9697f/-/format/auto/"
            alt="BoopMark Logo"
            className="w-10 h-10"
          />
          <span className="font-bricolage text-2xl font-bold text-[#0E1B36] dark:text-white">
            BoopMark
          </span>
        </div>
        
        {loading ? (
          <div>
            <h1 className="font-bricolage text-xl font-bold text-gray-900 dark:text-white mb-4">
              Signing you out...
            </h1>
            <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-[#7D65FF] mx-auto"></div>
          </div>
        ) : (
          <div>
            <h1 className="font-bricolage text-xl font-bold text-gray-900 dark:text-white mb-4">
              Sign Out
            </h1>
            <p className="font-inter text-gray-600 dark:text-gray-400 mb-6">
              Are you sure you want to sign out of your account?
            </p>
            <div className="space-y-3">
              <button
                onClick={handleSignOut}
                disabled={loading}
                className="w-full bg-[#7D65FF] dark:bg-[#8B7AFF] hover:bg-[#6B56E8] dark:hover:bg-[#7A6AEF] text-white font-inter font-semibold px-6 py-3 rounded-lg disabled:opacity-50 transition-colors"
              >
                {loading ? "Signing out..." : "Sign Out"}
              </button>
              <a
                href="/"
                className="block w-full text-center px-6 py-3 text-gray-700 dark:text-gray-300 border border-gray-300 dark:border-gray-600 rounded-lg hover:bg-gray-50 dark:hover:bg-[#1E1E1E] font-inter font-medium transition-colors"
              >
                Cancel
              </a>
            </div>
          </div>
        )}
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