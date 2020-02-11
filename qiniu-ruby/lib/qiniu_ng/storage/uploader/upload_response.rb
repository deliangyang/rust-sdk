# frozen_string_literal: true

require 'ffi'
require 'json'

module QiniuNg
  module Storage
    class Uploader
      class UploadResponse
        def initialize(upload_response_ffi)
          @upload_response = upload_response_ffi
          @cache = {}
        end
        private_class_method :new

        def hash
          core_ffi = Bindings.const_get :CoreFFI
          data = FFI::MemoryPointer.new(256)
          data_len = core_ffi::Size.new
          @upload_response.get_hash(data, data_len)
          return nil if data_len[:value].zero?
          data.read_string_length(data_len[:value])
        end

        def key
          @cache[:key] ||= @upload_response.get_key
          return nil if @cache[:key].is_null
          @cache[:key].get_ptr
        end

        def as_json
          @cache[:json] ||= QiniuNg::Error.wrap_ffi_function do
                              @upload_response.get_json_string
                            end
          return nil if @cache[:json].is_null
          @cache[:json].get_ptr
        end

        def method_missing(method_name)
          @cache[:parsed_json] ||= JSON.load(as_json)
          if @cache[:parsed_json].has_key?(method_name.to_s)
            return @cache[:parsed_json][method_name.to_s]
          end
          super
        end

        def inspect
          "#<#{self.class.name}>"
        end
      end
    end
  end
end
